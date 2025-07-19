use crate::{
    CONFIG, DEFAULT_CHUNKS_EMBEDDING_FIELD, DEFAULT_CHUNKS_END_LINE_FIELD, DEFAULT_CHUNKS_ID_FIELD,
    DEFAULT_CHUNKS_PATH_FIELD, DEFAULT_CHUNKS_START_LINE_FIELD, subsystems::chunker::ArcTextChunk,
};
use arrow_array::{
    ArrayRef, FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    types::Float64Type,
};
use async_trait::async_trait;
use itertools::Itertools;
use lancedb::{
    Table,
    arrow::arrow_schema::{DataType, Field, Fields, Schema},
    table::{OptimizeAction, OptimizeOptions},
};
use miette::{IntoDiagnostic, Result};
use rig::{
    OneOrMany,
    embeddings::{Embedding, EmbeddingsBuilder},
};
use rig_fastembed::EmbeddingModel;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc::Receiver;
use tokio_graceful_shutdown::{FutureExt, IntoSubsystem, SubsystemHandle};
use tracing::{info, trace};

pub struct IndexerSubsystem {
    pub chunks_rx: Receiver<Option<ArcTextChunk>>,
    pub embedding_model: EmbeddingModel,
    pub ndims: usize,
    pub table: Table,
    pub first_chunks_scan: Arc<AtomicBool>,
    pub first_index_scan: Arc<AtomicBool>,
}

#[async_trait]
impl IntoSubsystem<miette::Report> for IndexerSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        trace!(
            "Start indexer with embedding model: {:?}",
            self.embedding_model.model.to_string()
        );
        let mut embeddings = EmbeddingsBuilder::new(self.embedding_model.clone());
        let mut batch: Vec<ArcTextChunk> = Vec::new();

        trace!("Waiting for chunks");
        while let Some(chunk) = self.chunks_rx.recv().cancel_on_shutdown(&subsys).await? {
            if let Some(chunk) = chunk.as_ref() {
                trace!("Chunk received: {:?}", chunk.id);
                batch.push(chunk.clone());
            } else {
                trace!("Last chunk marker received");
            }
            trace!("Batch size before batching: {}", batch.len());
            if batch.len() == CONFIG.search.semantic.batch_size
                || (chunk.is_none() && !batch.is_empty())
            {
                trace!("Batch size reached, deleting old records");
                let ids = batch
                    .iter()
                    .format_with(",", |chunk, f| {
                        f(&format_args!(r#""{}""#, chunk.id.to_hash()))
                    })
                    .to_string();

                self.table
                    .delete(&format!("id in ({})", ids))
                    .await
                    .into_diagnostic()?;

                trace!("Embedding documents");
                embeddings = embeddings
                    .documents(batch.iter().cloned())
                    .into_diagnostic()?;

                let prepared_embeddings = embeddings.build().await.into_diagnostic()?;
                embeddings = EmbeddingsBuilder::new(self.embedding_model.clone());

                trace!("Building record batch");
                let records_batch = as_record_batch(prepared_embeddings, self.ndims);

                trace!("Adding record batch to table");
                let record_batch_iter =
                    RecordBatchIterator::new(vec![records_batch], Arc::new(schema(self.ndims)));

                self.table
                    .add(record_batch_iter)
                    .execute()
                    .await
                    .into_diagnostic()?;

                batch.clear();
            }

            trace!("Batch size after batching: {}", batch.len());

            //TODO: For POC purposes it always will be fully reindexed after first chunks scan, but need to reindex after all files are processed
            if self.first_chunks_scan.load(Ordering::Relaxed)
                && self.chunks_rx.is_empty()
                && chunk.is_none()
            {
                info!("Optimizing index after all chunks are processed");
                self.table
                    .optimize(OptimizeAction::Index(OptimizeOptions::default()))
                    .await
                    .into_diagnostic()?;
                trace!("Index optimized, setting first index scan to true");
                self.first_index_scan.store(true, Ordering::Relaxed);
            }
        }
        info!("Indexer finished");
        Ok(())
    }
}

pub fn schema(dims: usize) -> Schema {
    Schema::new(Fields::from(vec![
        Field::new(DEFAULT_CHUNKS_ID_FIELD, DataType::Utf8, false),
        Field::new(DEFAULT_CHUNKS_PATH_FIELD, DataType::Utf8, false),
        Field::new(DEFAULT_CHUNKS_START_LINE_FIELD, DataType::Int64, false),
        Field::new(DEFAULT_CHUNKS_END_LINE_FIELD, DataType::Int64, false),
        Field::new(
            DEFAULT_CHUNKS_EMBEDDING_FIELD,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float64, true)),
                dims as i32,
            ),
            false,
        ),
    ]))
}

pub fn as_record_batch(
    records: Vec<(ArcTextChunk, OneOrMany<Embedding>)>,
    dims: usize,
) -> Result<RecordBatch, lancedb::arrow::arrow_schema::ArrowError> {
    let ids = StringArray::from_iter_values(records.iter().map(|(chunk, _)| chunk.id.to_hash()));

    let paths = StringArray::from_iter_values(
        records
            .iter()
            .map(|(chunk, _)| chunk.path.to_string_lossy().to_string()),
    );

    let start_lines =
        Int64Array::from_iter_values(records.iter().map(|(chunk, _)| chunk.start_line as i64));

    let end_lines =
        Int64Array::from_iter_values(records.iter().map(|(chunk, _)| chunk.end_line as i64));

    let embedding = FixedSizeListArray::from_iter_primitive::<Float64Type, _, _>(
        records.iter().map(|(_, embeddings)| {
            Some(
                embeddings
                    .first()
                    .vec
                    .into_iter()
                    .map(Some)
                    .collect::<Vec<_>>(),
            )
        }),
        dims as i32,
    );

    RecordBatch::try_from_iter(vec![
        (DEFAULT_CHUNKS_ID_FIELD, Arc::new(ids) as ArrayRef),
        (DEFAULT_CHUNKS_PATH_FIELD, Arc::new(paths) as ArrayRef),
        (
            DEFAULT_CHUNKS_START_LINE_FIELD,
            Arc::new(start_lines) as ArrayRef,
        ),
        (
            DEFAULT_CHUNKS_END_LINE_FIELD,
            Arc::new(end_lines) as ArrayRef,
        ),
        (
            DEFAULT_CHUNKS_EMBEDDING_FIELD,
            Arc::new(embedding) as ArrayRef,
        ),
    ])
}
