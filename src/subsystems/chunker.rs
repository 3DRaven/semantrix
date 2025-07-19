use crate::{CONFIG, repositories::delete_by_path, services::SymbolInfo};
use async_trait::async_trait;
use derive_more::{Deref, DerefMut};
use lancedb::Table;
use miette::{IntoDiagnostic, Result, miette};
use rig::{
    Embed,
    embeddings::{EmbedError, TextEmbedder},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fmt::Display,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc::{Receiver, Sender},
};
use tokio_graceful_shutdown::{FutureExt, IntoSubsystem, SubsystemHandle};
use tracing::{info, trace, warn};
use wax::Glob;

use super::watcher::PathEvent;

pub struct ChunkerSubsystem {
    pub table: Table,
    pub path_event_rx: Receiver<Arc<PathEvent>>,
    pub chunks_tx: Sender<Option<ArcTextChunk>>,
    pub first_path_scan: Arc<AtomicBool>,
    pub first_chunks_scan: Arc<AtomicBool>,
}

impl ChunkerSubsystem {
    async fn process_file(&self, path: &Path) -> Result<()> {
        trace!("File found for chunking: {}", path.display());
        let file = File::open(path).await.into_diagnostic()?;
        trace!("File opened for chunking: {}", path.display());
        let mut reader = BufReader::new(file).lines();
        trace!("File reader created for chunking: {}", path.display());
        let mut text_chunk = TextChunk::new(path.to_path_buf().into(), 0);
        trace!("Text chunk created for chunking: {}", path.display());

        loop {
            let line = reader.next_line().await.ok().flatten();
            if let Some(line) = line {
                text_chunk.push_line(line);
                if text_chunk.is_full() {
                    trace!("Chunk is full, sending to indexer: {}", text_chunk.id);
                    self.chunks_tx
                        .send(Some(ArcTextChunk(Arc::new(text_chunk.clone()))))
                        .await
                        .into_diagnostic()?;
                    text_chunk = text_chunk.next_chunk();
                }
            } else {
                trace!(
                    "File reader finished, sending last chunk to indexer: {}",
                    text_chunk.id
                );
                if !text_chunk.is_empty() {
                    text_chunk.crop_last_chunk();
                    self.chunks_tx
                        .send(Some(ArcTextChunk(Arc::new(text_chunk.clone()))))
                        .await
                        .into_diagnostic()?;
                }
                trace!("Sending last chunk marker to indexer");
                self.chunks_tx.send(None).await.into_diagnostic()?;
                break;
            }
        }
        if self.first_path_scan.load(Ordering::Relaxed) && self.path_event_rx.is_empty() {
            if let Ok(false) = self.first_chunks_scan.compare_exchange(
                false,
                true,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                info!("First chunks scan set to true");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl IntoSubsystem<miette::Report> for ChunkerSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        info!("Start chunker");
        while let Some(event) = self
            .path_event_rx
            .recv()
            .cancel_on_shutdown(&subsys)
            .await?
        {
            //TODO: For POC purposes it always will be fully rechunked after each file modified, but need to rechunk only changed chunks
            if event.kind.is_remove() {
                trace!("File/folder removed: {:?}", event);
                delete_by_path(&self.table, event.path.as_ref()).await?;
            } else if event.kind.is_create() || event.kind.is_modify() {
                trace!("File/folder created/modified: {:?}", event);
                delete_by_path(&self.table, event.path.as_ref()).await?;
                if event.path.is_file() {
                    self.process_file(&event.path).await?;
                } else if event.path.is_dir() {
                    let positive =
                        Glob::new(CONFIG.search.semantic.pattern.as_str()).into_diagnostic()?;
                    let walker = positive.walk(event.path.as_ref());
                    for entry in walker
                        .filter_map(|it| it.ok())
                        .filter(|it| it.file_type().is_file())
                    {
                        self.process_file(entry.path()).await?;
                    }
                }
            } else {
                warn!("Skipping event: {:?}", event);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Deref, DerefMut)]
pub struct ArcTextChunk(Arc<TextChunk>);

impl Embed for ArcTextChunk {
    fn embed(&self, embedder: &mut TextEmbedder) -> Result<(), EmbedError> {
        self.text.iter().for_each(|s| {
            embedder.embed(s.to_string());
        });
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentPointer {
    Chunk(ChunkId),
    Symbol(SymbolInfo),
}

impl PartialOrd for DocumentPointer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DocumentPointer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (DocumentPointer::Chunk(left), DocumentPointer::Chunk(right)) => {
                left.start_line.cmp(&right.start_line)
            }
            (DocumentPointer::Symbol(left), DocumentPointer::Symbol(right)) => left
                .location
                .range
                .start
                .line
                .cmp(&right.location.range.start.line),
            (DocumentPointer::Chunk(left), DocumentPointer::Symbol(right)) => left
                .start_line
                .cmp(&(right.location.range.start.line as usize)),
            (DocumentPointer::Symbol(left), DocumentPointer::Chunk(right)) => {
                (left.location.range.start.line as usize).cmp(&right.start_line)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq, Hash)]
pub struct ChunkId {
    pub path: Arc<PathBuf>,
    pub start_line: usize,
    pub end_line: usize,
}

impl<'de> Deserialize<'de> for ChunkId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            id: String,
            path: Arc<PathBuf>,
            start_line: usize,
            end_line: usize,
        }

        let helper = Helper::deserialize(deserializer)?;

        let chunk_id = ChunkId {
            path: helper.path.clone(),
            start_line: helper.start_line,
            end_line: helper.end_line,
        };
        let computed_hash = chunk_id.to_hash();

        if helper.id != computed_hash {
            return Err(serde::de::Error::custom(miette!(
                "ChunkId hash mismatch: expected {}, got {}",
                computed_hash,
                helper.id
            )));
        }

        Ok(chunk_id)
    }
}

impl ChunkId {
    pub fn new(path: Arc<PathBuf>, start_line: usize, end_line: usize) -> Self {
        Self {
            path,
            start_line,
            end_line,
        }
    }

    pub fn to_hash(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish().to_string()
    }
}

impl Display for ChunkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}:{}:{}",
            self.to_hash(),
            self.path.display(),
            self.start_line,
            self.end_line
        )
    }
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct TextChunk {
    pub id: ChunkId,
    pub path: Arc<PathBuf>,
    pub start_line: usize,
    pub end_line: usize,
    pub text: Vec<String>,
}

impl TextChunk {
    pub fn new(path: Arc<PathBuf>, start_line: usize) -> Self {
        let end_line = start_line + CONFIG.search.semantic.chunk_size;
        Self {
            id: ChunkId::new(path.clone(), start_line, end_line),
            path,
            start_line,
            end_line,
            text: Vec::new(),
        }
    }

    pub fn crop_last_chunk(&mut self) {
        self.end_line = self.start_line + self.text.len();
    }

    pub fn is_full(&self) -> bool {
        self.text.len() == CONFIG.search.semantic.chunk_size
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn push_line(&mut self, line: String) {
        self.text.push(line);
    }

    pub fn count_lines(&self) -> usize {
        self.text.len()
    }

    pub fn next_chunk(&self) -> TextChunk {
        let mut next_chunk = TextChunk::new(
            self.path.clone(),
            self.end_line - CONFIG.search.semantic.overlap_size,
        );
        let tail = &self.text[self
            .text
            .len()
            .saturating_sub(CONFIG.search.semantic.overlap_size)..];
        next_chunk.text.extend_from_slice(tail);
        next_chunk
    }
}
