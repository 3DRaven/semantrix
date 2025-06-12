use std::sync::{Arc, atomic::AtomicBool};

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use rig_fastembed::EmbeddingModel;
use rig_lancedb::LanceDbVectorIndex;
use rmcp::{ServiceExt, service::RunningService, transport};
use tokio::sync::watch::Receiver;
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tracing::{error, info};

use crate::{services::CodeReuseSearchService, subsystems::lsp::GuardedLspServer};

pub struct McpServerSubsystem {
    pub vector_store: Arc<LanceDbVectorIndex<EmbeddingModel>>,
    pub lsp_server_rx: Receiver<Option<GuardedLspServer>>,
    pub first_index_scan: Arc<AtomicBool>,
}

#[async_trait]
impl IntoSubsystem<miette::Report> for McpServerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let service = CodeReuseSearchService {
            vector_store: self.vector_store.clone(),
            lsp_server_rx: self.lsp_server_rx,
            first_index_scan: self.first_index_scan.clone(),
        };
        info!("Starting MCP service");
        let cancelation_token = subsys.create_cancellation_token();
        let server: RunningService<_, _> = service
            .serve_with_ct(transport::stdio(), cancelation_token)
            .await
            .inspect_err(|e| error!("MCP server error: {:?}", e))
            .into_diagnostic()?;
        info!("MCP server initialized");
        let quit_reason = server.waiting().await.into_diagnostic()?;
        info!("MCP server shutdown with reason: {:?}", quit_reason);
        Ok(())
    }
}
