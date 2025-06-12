use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use log::info;
use miette::{IntoDiagnostic, Result};
use semantrix::{
    CONFIG, init_db, init_logger,
    subsystems::{
        chunker::ChunkerSubsystem, indexer::IndexerSubsystem, lsp::LspServerSubsystem,
        mcp::McpServerSubsystem, watcher::WatcherSubsystem,
    },
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle, Toplevel};

#[tokio::main]
async fn main() -> Result<()> {
    let _log_guard = init_logger()?;
    info!(
        "Starting server in work directory: {}",
        std::env::current_dir().into_diagnostic()?.display()
    );
    let (lsp_server_tx, lsp_server_rx) = tokio::sync::watch::channel(None);
    let (path_event_tx, path_event_rx) = tokio::sync::mpsc::channel(CONFIG.channel_size);
    let (chunks_tx, chunks_rx) = tokio::sync::mpsc::channel(CONFIG.channel_size);

    let (ndims, table, embedding_model, vector_store) = init_db().await?;

    let first_path_scan = Arc::new(AtomicBool::new(false));
    let first_chunks_scan = Arc::new(AtomicBool::new(false));
    let first_index_scan = Arc::new(AtomicBool::new(false));

    let watcher = WatcherSubsystem {
        path_event_tx,
        first_path_scan: first_path_scan.clone(),
    };
    let chunker = ChunkerSubsystem {
        table: table.clone(),
        path_event_rx,
        chunks_tx,
        first_path_scan: first_path_scan.clone(),
        first_chunks_scan: first_chunks_scan.clone(),
    };
    let indexer = IndexerSubsystem {
        chunks_rx,
        ndims,
        table: table.clone(),
        embedding_model: embedding_model.clone(),
        first_chunks_scan: first_chunks_scan.clone(),
        first_index_scan: first_index_scan.clone(),
    };
    let lsp_server = LspServerSubsystem { lsp_server_tx };
    let mcp_server = McpServerSubsystem {
        vector_store: vector_store.clone(),
        lsp_server_rx,
        first_index_scan: first_index_scan.clone(),
    };
    Toplevel::new(
        |s: SubsystemHandle<Box<dyn std::error::Error + Send + Sync>>| async move {
            s.start(SubsystemBuilder::new("Watcher", watcher.into_subsystem()));
            s.start(SubsystemBuilder::new("Chunker", chunker.into_subsystem()));
            s.start(SubsystemBuilder::new("Indexer", indexer.into_subsystem()));
            s.start(SubsystemBuilder::new(
                "LSP server",
                lsp_server.into_subsystem(),
            ));
            s.start(SubsystemBuilder::new(
                "MCP server",
                mcp_server.into_subsystem(),
            ));
        },
    )
    .catch_signals()
    .handle_shutdown_requests(Duration::from_millis(CONFIG.shutdown_timeout))
    .await
    .map_err(Into::into)
    .inspect(|_| info!("Finall message"))
    .inspect_err(|e| info!("Final message in error case: {:?}", e))
}
