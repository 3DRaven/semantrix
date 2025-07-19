use std::{
    collections::HashSet,
    path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::StreamExt;
use miette::Result;
use rig_fastembed::EmbeddingModel;
use rig_lancedb::LanceDbVectorIndex;
use rmcp::{
    Error, ServerHandler,
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    tool,
};
use schemars::{
    JsonSchema, SchemaGenerator,
    schema::{InstanceType, ObjectValidation, Schema, SchemaObject},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch::{self};
use tracing::{debug, error, info};

use crate::services::{
    Ruleset, SymbolPlaceTo, find_max_distance_paths, find_min_distance_paths,
    get_documents_symbols, get_fuzzy_symbols, get_semantic_symbols, get_symbols_references,
    most_common_parent,
};
use crate::{CONFIG, NAME, ResponseType, TERA, VERSION, subsystems::lsp::GuardedLspServer};

#[derive(Debug, Deserialize, Serialize)]
pub struct CodeReuseSearchRequest {
    pub semantic_queries: Vec<String>,
    pub name_patterns: Vec<String>,
}

impl JsonSchema for CodeReuseSearchRequest {
    fn schema_name() -> String {
        "CodeReuseSearchRequest".to_owned()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut context = tera::Context::new();
        context.insert("name", &NAME);
        context.insert("version", &VERSION);
        let semantic_queries_desc = TERA
            .render(
                &CONFIG.templates.description.semantic_query.clone(),
                &context,
            )
            .expect("Failed to render template");

        let name_patterns_desc = TERA
            .render(&CONFIG.templates.description.fuzzy_query.clone(), &context)
            .expect("Failed to render template");

        let mut semantic_queries_schema = generator.subschema_for::<Vec<String>>();
        if let Schema::Object(ref mut obj) = semantic_queries_schema {
            obj.metadata().description = Some(semantic_queries_desc.to_string());
        }

        let mut name_patterns_schema = generator.subschema_for::<Vec<String>>();
        if let Schema::Object(ref mut obj) = name_patterns_schema {
            obj.metadata().description = Some(name_patterns_desc.to_string());
        }

        let schema_obj = SchemaObject {
            instance_type: Some(InstanceType::Object.into()),
            object: Some(Box::new(ObjectValidation {
                properties: [
                    ("semantic_queries".to_string(), semantic_queries_schema),
                    ("name_patterns".to_string(), name_patterns_schema),
                ]
                .iter()
                .cloned()
                .collect(),
                required: vec!["semantic_queries".to_string(), "name_patterns".to_string()]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        };

        Schema::Object(schema_obj)
    }
}

#[derive(Clone)]
pub struct McpService {
    pub vector_store: Arc<LanceDbVectorIndex<EmbeddingModel>>,
    pub lsp_server_rx: watch::Receiver<Option<GuardedLspServer>>,
    pub first_index_scan: Arc<AtomicBool>,
}

#[tool(tool_box)]
impl McpService {
    #[tool(
        description = "A tool that scans your project to identify symbols and place them to the best place in the project"
    )]
    pub async fn symbols_placer(&self) -> Result<CallToolResult, Error> {
        let lsp_server = if let Some(lsp_server) = self.lsp_server_rx.borrow().clone() {
            lsp_server
        } else {
            return Ok(CallToolResult::error(vec![Content::text(
                "Waiting for LSP server to be initialized".to_string(),
            )]));
        };

        info!("Starting to get symbols");

        let modules_symbols = get_fuzzy_symbols(
            &lsp_server,
            vec![],
            Some(CONFIG.placer.prefetch_symbol_kinds.clone()),
            false,
        )
        .await
        .inspect_err(|e| {
            error!("Error getting symbols: {}", e);
        })
        .map_err(|e| Error::internal_error(format!("Failed to get symbols: {}", e), None))?
        .into_iter()
        .map(|it| it.location.uri)
        .collect::<HashSet<_>>();

        debug!("Found modules symbols: {:?}", modules_symbols);

        let symbols = get_documents_symbols(&lsp_server, modules_symbols, true)
            .collect::<Vec<_>>()
            .await;

        debug!("Found symbols: {:?}", symbols);

        let places: Vec<SymbolPlaceTo> = get_symbols_references(&lsp_server, symbols.clone())
            .filter_map(|it| async move {
                let candidates = it
                    .references
                    .iter()
                    .filter_map(|it| it.uri.to_file_path().ok())
                    .map(|it| it.to_path_buf())
                    .map(|it| path::absolute(it).unwrap())
                    .collect::<Vec<_>>();

                let place_to = if CONFIG.placer.use_max_distance {
                    find_max_distance_paths(&candidates, &candidates)
                } else {
                    find_min_distance_paths(&candidates, &candidates)
                };

                if place_to.is_empty() {
                    None
                } else {
                    //If lot of places to place, we need to find the closest parent includes all places
                    let absolute_target = if place_to.len() > 1 {
                        most_common_parent(&place_to).unwrap_or(
                            place_to
                                .first()
                                .and_then(|it| path::absolute(it).ok())
                                .unwrap()
                                .parent()
                                .unwrap()
                                .to_path_buf(),
                        )
                    } else {
                        place_to
                            .first()
                            .and_then(|it| path::absolute(it).ok())
                            .unwrap()
                            .parent()
                            .unwrap()
                            .to_path_buf()
                    };

                    debug!(
                        "For symbol: {:?} absolute target: {}",
                        it.symbol_info,
                        absolute_target.display()
                    );

                    if let Ok(path) = it.symbol_info.location.uri.to_file_path() {
                        if let Ok(absolute_source) = path::absolute(&path) {
                            if let Some(source_parent) = absolute_source.parent() {
                                if source_parent == absolute_target {
                                    None
                                } else {
                                    Some(SymbolPlaceTo {
                                        symbol_info: it.symbol_info,
                                        place_to: absolute_target.to_string_lossy().to_string(),
                                    })
                                }
                            } else {
                                Some(SymbolPlaceTo {
                                    symbol_info: it.symbol_info,
                                    place_to: absolute_target.to_string_lossy().to_string(),
                                })
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
            .await;

        debug!("Places: {:?}", places);

        // TODO: for POC loaded every request because user can update rules without restarting the server
        let rules: Ruleset =
            serde_yaml::from_reader(std::fs::File::open(&CONFIG.rules).map_err(|e| {
                Error::internal_error(
                    format!(
                        "Failed to open rules file: {} with path: {}",
                        e,
                        &CONFIG.rules.to_string_lossy()
                    ),
                    None,
                )
            })?)
            .map_err(|e| {
                Error::internal_error(
                    format!(
                        "Failed to parse rules file: {} with path: {}",
                        e,
                        &CONFIG.rules.to_string_lossy()
                    ),
                    None,
                )
            })?;

        let rules = rules.get_rules(symbols.clone()).map_err(|e| {
            Error::internal_error(
                format!(
                    "Failed to get fuzzy rules: {} with path: {}",
                    e,
                    &CONFIG.rules.to_string_lossy()
                ),
                None,
            )
        })?;

        if CONFIG.response == ResponseType::Json {
            Ok(CallToolResult::success(vec![
                Content::json(rules)?,
                Content::json(symbols)?,
                Content::json(places)?,
            ]))
        } else {
            let mut context = tera::Context::new();
            context.insert("fuzzy_rules", &rules);
            context.insert("fuzzy_symbols", &symbols);
            context.insert("references", &places);

            let content = TERA
                .render(&CONFIG.templates.prompts.placer, &context)
                .map_err(|e| {
                    Error::internal_error(
                        format!(
                            "Failed to render template: {} with path: {}",
                            e, &CONFIG.templates.prompts.placer
                        ),
                        None,
                    )
                })?;
            Ok(CallToolResult::success(vec![Content::text(content)]))
        }
    }

    #[tool(
        description = "A tool that scans your project to identify code fragments and components that have already been implemented, allowing you to find and reuse existing solutions instead of rewriting them from scratch. This helps reduce duplication, improve development efficiency, and promote best practices in code maintenance and organization"
    )]
    pub async fn code_reuse_search(
        &self,
        #[tool(aggr)] CodeReuseSearchRequest {
            semantic_queries,
            name_patterns,
        }: CodeReuseSearchRequest,
    ) -> Result<CallToolResult, Error> {
        let lsp_server = if let Some(lsp_server) = self.lsp_server_rx.borrow().clone() {
            lsp_server
        } else {
            return Ok(CallToolResult::error(vec![Content::text(
                "Waiting for LSP server to be initialized".to_string(),
            )]));
        };

        if !self.first_index_scan.load(Ordering::Relaxed) {
            return Ok(CallToolResult::error(vec![Content::text(
                "Waiting for index to be initialized".to_string(),
            )]));
        }

        info!("Starting to get symbols");

        let (fuzzy_symbols, semantic_symbols) = tokio::try_join!(
            get_fuzzy_symbols(&lsp_server, name_patterns, None, true),
            get_semantic_symbols(&lsp_server, semantic_queries, self.vector_store.clone(),),
        )
        .inspect_err(|e| {
            error!("Error getting symbols: {}", e);
        })
        .map_err(|e| Error::internal_error(format!("Failed to get symbols: {}", e), None))?;

        debug!(
            "Fuzzy symbols: {:?}, semantic symbols: {:?}",
            fuzzy_symbols, semantic_symbols
        );

        // TODO: for POC loaded every request because user can update rules without restarting the server
        let rules: Ruleset =
            serde_yaml::from_reader(std::fs::File::open(&CONFIG.rules).map_err(|e| {
                Error::internal_error(
                    format!(
                        "Failed to open rules file: {} with path: {}",
                        e,
                        &CONFIG.rules.to_string_lossy()
                    ),
                    None,
                )
            })?)
            .map_err(|e| {
                Error::internal_error(
                    format!(
                        "Failed to parse rules file: {} with path: {}",
                        e,
                        &CONFIG.rules.to_string_lossy()
                    ),
                    None,
                )
            })?;

        let semantic_rules = rules.get_rules(semantic_symbols.clone()).map_err(|e| {
            Error::internal_error(
                format!(
                    "Failed to get semantic rules: {} with path: {}",
                    e,
                    &CONFIG.rules.to_string_lossy()
                ),
                None,
            )
        })?;
        let fuzzy_rules = rules.get_rules(fuzzy_symbols.clone()).map_err(|e| {
            Error::internal_error(
                format!(
                    "Failed to get fuzzy rules: {} with path: {}",
                    e,
                    &CONFIG.rules.to_string_lossy()
                ),
                None,
            )
        })?;

        if CONFIG.response == ResponseType::Json {
            Ok(CallToolResult::success(vec![
                Content::json(semantic_rules)?,
                Content::json(fuzzy_rules)?,
                Content::json(semantic_symbols)?,
                Content::json(fuzzy_symbols)?,
            ]))
        } else {
            let mut context = tera::Context::new();
            context.insert("semantic_rules", &semantic_rules);
            context.insert("fuzzy_rules", &fuzzy_rules);
            context.insert("semantic_symbols", &semantic_symbols);
            context.insert("fuzzy_symbols", &fuzzy_symbols);

            let content = TERA
                .render(&CONFIG.templates.prompts.searcher, &context)
                .map_err(|e| {
                    Error::internal_error(
                        format!(
                            "Failed to render template: {} with path: {}",
                            e, &CONFIG.templates.prompts.searcher
                        ),
                        None,
                    )
                })?;
            Ok(CallToolResult::success(vec![Content::text(content)]))
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        let mut context = tera::Context::new();
        context.insert("name", &NAME);
        context.insert("version", &VERSION);
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                TERA.render(&CONFIG.templates.description.server.clone(), &context)
                    .expect("Failed to render template"),
            ),
        }
    }
}
