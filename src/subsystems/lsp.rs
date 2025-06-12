use crate::{CONFIG, enums::McpProgressToken};
use async_lsp_client::{LspServer, ServerMessage};
use async_trait::async_trait;
use lsp_types::{
    ClientCapabilities, ClientInfo, DocumentSymbolClientCapabilities, DocumentSymbolParams,
    DocumentSymbolResponse, InitializeParams, NumberOrString, PartialResultParams, ProgressParams,
    ProgressParamsValue, SymbolKind, SymbolKindCapability, TextDocumentClientCapabilities,
    TextDocumentIdentifier, Url, WindowClientCapabilities, WorkDoneProgress,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceFolder,
    WorkspaceSymbolClientCapabilities, WorkspaceSymbolParams, WorkspaceSymbolResponse,
    request::{
        DocumentSymbolRequest, Request, Shutdown, WorkDoneProgressCreate, WorkspaceSymbolRequest,
    },
};
use miette::{IntoDiagnostic, Result};
use std::{path::Path, str::FromStr, sync::Arc};
use tokio::sync::{Semaphore, mpsc, watch::Sender};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tower_lsp::jsonrpc::{self};
use tracing::{debug, error, info, trace, warn};

use crate::{NAME, VERSION};

#[derive(Clone)]
pub struct GuardedLspServer {
    server: LspServer,
    guard: Arc<Semaphore>,
}

impl GuardedLspServer {
    pub async fn shutdown(&self) -> Result<()> {
        let _permit = self.guard.acquire().await.into_diagnostic()?;
        info!("Shutting down LSP server");
        self.server.shutdown().await.into_diagnostic()?;
        info!("Exiting LSP server");
        self.server.exit().await;
        info!("LSP server shutdown");
        Ok(())
    }

    pub async fn send_workspace_symbol_request(
        &self,
        query: String,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        if let Err(e) = self.guard.try_acquire() {
            warn!("LSP server is busy: {:?}", e);
            let _permit = self.guard.acquire().await.into_diagnostic()?;
        }
        info!("Sending workspace symbol request: {}", query);
        self.server
            .send_request::<WorkspaceSymbolRequest>(WorkspaceSymbolParams {
                query,
                ..Default::default()
            })
            .await
            .inspect(|it| {
                info!("Workspace symbols response: {:?}", it);
            })
            .inspect_err(|e| {
                error!("Error sending workspace symbol request: {:?}", e);
            })
            .into_diagnostic()
    }

    pub async fn send_document_symbol_request(
        &self,
        document_uri: Url,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.server
            .send_request::<DocumentSymbolRequest>(DocumentSymbolParams {
                text_document: TextDocumentIdentifier::new(document_uri.clone()),
                work_done_progress_params: WorkDoneProgressParams {
                    work_done_token: None,
                },
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .inspect(|it| {
                info!("Document symbols response: {:?}", it);
            })
            .inspect_err(|e| {
                error!("Error sending document symbol request: {:?}", e);
            })
            .into_diagnostic()
    }
}
pub struct LspServerSubsystem {
    pub lsp_server_tx: Sender<Option<GuardedLspServer>>,
}

#[async_trait]
impl IntoSubsystem<miette::Report> for LspServerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let server_args = CONFIG
            .search
            .fuzzy
            .server_args
            .iter()
            .collect::<Vec<&String>>();

        let (server, rx) = LspServer::new(&CONFIG.search.fuzzy.lsp_server, server_args);

        let workspace_path = Path::new(&CONFIG.search.fuzzy.workspace_uri);

        let workspace_name = workspace_path
            .file_name()
            .expect("Failed to get workspace folder")
            .to_str()
            .expect("Failed to convert workspace folder to string");

        let initialize_params = InitializeParams {
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    symbol: Some(WorkspaceSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        symbol_kind: Some(SymbolKindCapability {
                            value_set: Some(vec![
                                SymbolKind::FILE,
                                SymbolKind::MODULE,
                                SymbolKind::NAMESPACE,
                                SymbolKind::PACKAGE,
                                SymbolKind::CLASS,
                                SymbolKind::METHOD,
                                SymbolKind::PROPERTY,
                                SymbolKind::FIELD,
                                SymbolKind::CONSTRUCTOR,
                                SymbolKind::ENUM,
                                SymbolKind::INTERFACE,
                                SymbolKind::FUNCTION,
                                SymbolKind::VARIABLE,
                                SymbolKind::CONSTANT,
                                SymbolKind::STRING,
                                SymbolKind::NUMBER,
                                SymbolKind::BOOLEAN,
                                SymbolKind::ARRAY,
                                SymbolKind::OBJECT,
                                SymbolKind::KEY,
                                SymbolKind::NULL,
                                SymbolKind::ENUM_MEMBER,
                                SymbolKind::STRUCT,
                                SymbolKind::EVENT,
                                SymbolKind::OPERATOR,
                                SymbolKind::TYPE_PARAMETER,
                            ]),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                text_document: Some(TextDocumentClientCapabilities {
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        dynamic_registration: Some(false),
                        hierarchical_document_symbol_support: Some(false),
                        symbol_kind: Some(SymbolKindCapability {
                            value_set: Some(vec![
                                SymbolKind::FILE,
                                SymbolKind::MODULE,
                                SymbolKind::NAMESPACE,
                                SymbolKind::PACKAGE,
                                SymbolKind::CLASS,
                                SymbolKind::METHOD,
                                SymbolKind::PROPERTY,
                                SymbolKind::FIELD,
                                SymbolKind::CONSTRUCTOR,
                                SymbolKind::ENUM,
                                SymbolKind::INTERFACE,
                                SymbolKind::FUNCTION,
                                SymbolKind::VARIABLE,
                                SymbolKind::CONSTANT,
                                SymbolKind::STRING,
                                SymbolKind::NUMBER,
                                SymbolKind::BOOLEAN,
                                SymbolKind::ARRAY,
                                SymbolKind::OBJECT,
                                SymbolKind::KEY,
                                SymbolKind::NULL,
                                SymbolKind::ENUM_MEMBER,
                                SymbolKind::STRUCT,
                                SymbolKind::EVENT,
                                SymbolKind::OPERATOR,
                                SymbolKind::TYPE_PARAMETER,
                            ]),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            process_id: Some(std::process::id()),
            initialization_options: Some(CONFIG.search.fuzzy.server_options.clone()),
            client_info: Some(ClientInfo {
                name: NAME.to_string(),
                version: Some(VERSION.to_string()),
            }),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: Url::from_str(&CONFIG.search.fuzzy.workspace_uri)
                    .expect("Failed to parse workspace folder"),
                name: workspace_name.to_string(),
            }]),
            ..Default::default()
        };

        let initialize_result = server.initialize(initialize_params).await;
        info!("Initialize result: {:?}", initialize_result);
        server.initialized().await;
        //For all server requests, send a "Ok" response without any reaction
        fake_responder(&server, rx).await?;
        let guarded_server = GuardedLspServer {
            server: server.clone(),
            guard: Arc::new(Semaphore::new(CONFIG.search.fuzzy.parallelizm)),
        };
        self.lsp_server_tx
            .send(Some(guarded_server.clone()))
            .into_diagnostic()?;
        subsys.on_shutdown_requested().await;
        guarded_server.shutdown().await?;
        Ok(())
    }
}

pub async fn fake_responder(
    server: &LspServer,
    mut rx: mpsc::Receiver<ServerMessage>,
) -> Result<()> {
    info!("Waiting for indexing to complete");
    wait_completion(&mut rx, Some(McpProgressToken::RootsScanned)).await?;
    let server = server.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            match &message {
                ServerMessage::Notification(notification) => {
                    trace!("Received notification: {:?}", notification);
                }
                ServerMessage::Request(request) => {
                    trace!("Received request: {:?}", request);
                    if let Some(id) = request.id() {
                        match request.method() {
                            WorkDoneProgressCreate::METHOD => {
                                trace!("Sending response for request: {:?}", request);
                                server
                                    .send_response::<WorkDoneProgressCreate>(id.clone(), ())
                                    .await;
                                continue;
                            }
                            Shutdown::METHOD => {
                                debug!("Sending response for shutdown request: {:?}", request);
                                server.send_response::<Shutdown>(id.clone(), ()).await;
                                continue;
                            }
                            _ => {
                                warn!("Sending error response for request: {:?}", request);
                                server
                                    .send_error_response(
                                        id.clone(),
                                        jsonrpc::Error {
                                            code: jsonrpc::ErrorCode::MethodNotFound,
                                            message: std::borrow::Cow::Borrowed("Method Not Found"),
                                            data: request.params().cloned(),
                                        },
                                    )
                                    .await;
                            }
                        }
                    } else {
                        warn!("Received request with no id");
                    }
                }
            }
        }
        info!("Server message receiver closed");
    });
    Ok(())
}

pub async fn wait_completion(
    rx: &mut mpsc::Receiver<ServerMessage>,
    token: Option<McpProgressToken>,
) -> Result<()> {
    if let Some(token) = token.as_ref() {
        info!("Waiting for work done of {:?}", token);
        while let Some(message) = rx.recv().await {
            if let ServerMessage::Notification(notification) = &message {
                trace!("Notification: {:?}", notification);
                if notification.method == "$/progress" {
                    if let Some(params) = notification.params.clone() {
                        let params: ProgressParams =
                            serde_json::from_value(params).into_diagnostic()?;
                        if params.token == NumberOrString::String(token.to_string()) {
                            if let ProgressParamsValue::WorkDone(WorkDoneProgress::End(message)) =
                                params.value
                            {
                                info!("Work done with message: {:?}", message);
                                break;
                            }
                        }
                    }
                }
            } else if let ServerMessage::Request(request) = &message {
                trace!("Received request: {:?}", request);
            }
        }
    } else {
        info!("Waiting for work done");
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::Notification(notification) = &message {
                trace!("Received notification: {:?}", notification);
            } else if let ServerMessage::Request(request) = &message {
                trace!("Received request: {:?}", request);
            }
        }
        info!("Received all messages");
    }
    Ok(())
}
