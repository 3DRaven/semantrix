use std::hash::{Hash, Hasher};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::{Stream, StreamExt, TryStreamExt, future::Either, stream};
use itertools::Itertools;
use lsp_types::{
    DocumentSymbolResponse, Location, OneOf, Position, Range, WorkspaceSymbolResponse,
};
use miette::{IntoDiagnostic, Result};
use regex::RegexSet;
use rig::vector_store::VectorStoreIndexDyn;
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
use serde::{Deserialize, Deserializer, Serialize};
use tera::Tera;
use tokio::sync::watch::{self};
use tracing::{debug, error, info, trace};
use url::Url;
use wax::{Glob, Pattern};

use crate::{
    CONFIG, NAME, ResponseType, TERA, VERSION,
    subsystems::{
        chunker::{ChunkId, DocumentPointer},
        lsp::GuardedLspServer,
    },
};

#[derive(Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub location: Location,
    pub container_name: Option<String>,
    pub code: Option<String>,
}

impl SymbolInfo {
    pub fn path(&self) -> Result<PathBuf> {
        self.location
            .uri
            .to_file_path()
            .map_err(|_| miette::miette!("Failed to convert URL {} to path", self.location.uri))
    }
}

#[derive(Deserialize, Debug)]
pub struct Ruleset {
    pub common: Vec<String>,
    pub depends_on: Vec<SymbolRuleset>,
}

#[derive(Deserialize, Debug)]
pub struct SymbolRuleset {
    #[serde(deserialize_with = "deserialize_regexset")]
    pub kind: RegexSet,
    #[serde(deserialize_with = "deserialize_regexset")]
    pub name: RegexSet,
    pub path: Vec<String>,
    #[serde(deserialize_with = "deserialize_regexset")]
    pub code: RegexSet,
    pub rules: Vec<String>,
    #[serde(skip)]
    pub tera: Vec<Tera>,
}

impl PartialEq for SymbolRuleset {
    fn eq(&self, other: &Self) -> bool {
        self.kind.patterns() == other.kind.patterns()
            && self.name.patterns() == other.name.patterns()
            && self.path == other.path
            && self.code.patterns() == other.code.patterns()
    }
}

impl Eq for SymbolRuleset {}

impl Hash for SymbolRuleset {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.patterns().hash(state);
        self.name.patterns().hash(state);
        self.path.hash(state);
        self.code.patterns().hash(state);
    }
}

fn deserialize_regexset<'de, D>(deserializer: D) -> Result<RegexSet, D::Error>
where
    D: Deserializer<'de>,
{
    let patterns: Vec<String> = Vec::deserialize(deserializer)?;
    RegexSet::new(&patterns).map_err(serde::de::Error::custom)
}

impl SymbolRuleset {
    pub fn matches(&self, symbol_info: &SymbolInfo) -> Result<bool> {
        let path = symbol_info.path()?;
        let path_patterns = self
            .path
            .iter()
            .map(|pattern| Glob::new(pattern))
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()?;

        trace!("Kind: {:?}", self.kind.is_match(&symbol_info.kind));
        trace!("Name: {:?}", self.name.is_match(&symbol_info.name));
        trace!(
            "Path: {:?}",
            path_patterns
                .iter()
                .any(|pattern| pattern.is_match(path.as_path()))
        );
        trace!(
            "Code: {:?}",
            symbol_info
                .code
                .as_ref()
                .map(|code| self.code.is_match(code))
                .unwrap_or(false)
        );

        Ok(self.kind.is_match(&symbol_info.kind)
            && self.name.is_match(&symbol_info.name)
            && path_patterns
                .iter()
                .any(|pattern| pattern.is_match(path.as_path()))
            && symbol_info
                .code
                .as_ref()
                .map(|code| self.code.is_match(code))
                .unwrap_or(false))
    }
}

impl Ruleset {
    pub fn get_rules(&self, symbols: Vec<SymbolInfo>) -> Result<Vec<String>> {
        #[allow(clippy::mutable_key_type)]
        let mut matched: HashMap<&SymbolRuleset, Vec<&SymbolInfo>> = HashMap::new();
        let mut matches = self.common.clone();

        for rule in self.depends_on.iter() {
            trace!("Checking rule: {:?}", rule);
            for symbol in symbols.iter() {
                if rule.matches(symbol)? {
                    debug!("Matched rule for symbol: {:?}", symbol);
                    matched.entry(rule).or_default().push(symbol);
                } else {
                    trace!("Not matched rule for symbol: {:?}", symbol);
                }
            }
        }

        for (rule, symbols) in matched.into_iter() {
            let mut context = tera::Context::new();
            context.insert("symbols", &symbols);

            let semantic_queries_desc = rule
                .rules
                .iter()
                .map(|rule| Tera::one_off(rule, &context, true))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;

            matches.extend(semantic_queries_desc);
        }
        Ok(matches)
    }
}

impl std::fmt::Debug for SymbolInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{{}:{}:{}:{}}}",
            self.name, self.kind, self.location.uri, self.location.range.start.line
        )
    }
}

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
pub struct CodeReuseSearchService {
    pub vector_store: Arc<LanceDbVectorIndex<EmbeddingModel>>,
    pub lsp_server_rx: watch::Receiver<Option<GuardedLspServer>>,
    pub first_index_scan: Arc<AtomicBool>,
}

#[tool(tool_box)]
impl CodeReuseSearchService {
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
            get_fuzzy_symbols(&lsp_server, name_patterns),
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
                .render(&CONFIG.templates.prompt, &context)
                .map_err(|e| {
                    Error::internal_error(
                        format!(
                            "Failed to render template: {} with path: {}",
                            e, &CONFIG.templates.prompt
                        ),
                        None,
                    )
                })?;
            Ok(CallToolResult::success(vec![Content::text(content)]))
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for CodeReuseSearchService {
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

#[derive(Debug)]
pub struct DocumentSymbols {
    pub path: Url,
    pub symbols: DocumentSymbolResponse,
}

async fn get_fuzzy_symbols(
    lsp_server: &GuardedLspServer,
    possible_names: Vec<String>,
) -> Result<Vec<SymbolInfo>> {
    info!("Getting fuzzy symbols for: {:?}", possible_names);

    let symbols = get_workspace_symbols(lsp_server, possible_names)
        .flat_map(|response| match response {
            WorkspaceSymbolResponse::Flat(s) => {
                let stream = stream::iter(s).map(|symbol| {
                    let location = symbol.location.clone();
                    let code = get_code_from_document(location.uri, Some(location.range));
                    SymbolInfo {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind),
                        location: symbol.location,
                        container_name: symbol.container_name,
                        code,
                    }
                });

                Either::Left(stream)
            }
            WorkspaceSymbolResponse::Nested(s) => {
                let stream = stream::iter(s).map(|symbol| {
                    let code = match symbol.location.clone() {
                        OneOf::Left(location) => {
                            get_code_from_document(location.uri, Some(location.range))
                        }
                        OneOf::Right(location) => get_code_from_document(location.uri, None),
                    };

                    SymbolInfo {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind),
                        location: match symbol.location {
                            OneOf::Left(location) => location,
                            OneOf::Right(location) => Location::new(
                                location.uri,
                                Range::new(Position::new(0, 0), Position::new(0, 0)),
                            ),
                        },
                        container_name: symbol.container_name,
                        code,
                    }
                });
                Either::Right(stream)
            }
        })
        .collect::<Vec<_>>()
        .await;

    Ok(symbols)
}

async fn get_semantic_symbols(
    lsp_server: &GuardedLspServer,
    short_descriptions: Vec<String>,
    vector_store: Arc<LanceDbVectorIndex<EmbeddingModel>>,
) -> Result<Vec<SymbolInfo>> {
    info!("Getting semantic symbols for: {:?}", short_descriptions);
    let chunks = stream::iter(short_descriptions)
        .map(move |short_description| {
            let short_description = short_description.clone();
            let vector_store = vector_store.clone();
            async move {
                vector_store
                    .top_n(&short_description, CONFIG.search.semantic.search_limit)
                    .await
                    .map_err(|e| {
                        Error::internal_error(
                            format!("Failed to get semantic symbols: {}", e),
                            None,
                        )
                    })
            }
        })
        .filter_map(|it| async {
            it.await
                .inspect_err(|err| {
                    error!("Error getting symbols: {}", err);
                })
                .inspect(|it| {
                    info!("Semantic search result: {:?}", it);
                })
                .ok()
        })
        .flat_map(|it| {
            stream::iter(it).map(|(_, _, value)| {
                serde_json::from_value::<ChunkId>(value).inspect_err(|e| {
                    error!("Error parsing chunk id: {}", e);
                })
            })
        })
        .inspect_err(|err| {
            error!("Semantic search error: {}", err);
        })
        .filter_map(|it| async { it.ok() })
        .collect::<Vec<_>>()
        .await;

    trace!("Chunks: {:?}", chunks);

    let paths = chunks
        .iter()
        .map(|it| it.path.as_path())
        .map(Url::from_file_path)
        .filter_map(|it| it.ok())
        .collect::<HashSet<_>>();

    info!("Paths: {:?}", paths);

    let documents = get_documents_symbols(lsp_server, paths)
        .collect::<Vec<_>>()
        .await;

    trace!("Documents: {:?}", documents);

    let iter = chunks
        .into_iter()
        .map(DocumentPointer::Chunk)
        .chain(documents.into_iter().map(DocumentPointer::Symbol));

    let mut grouped: HashMap<PathBuf, Vec<DocumentPointer>> = HashMap::new();

    for pointer in iter {
        let key = match &pointer {
            DocumentPointer::Chunk(chunk) => chunk.path.as_path().to_path_buf(),
            DocumentPointer::Symbol(symbol) => {
                symbol.location.uri.to_file_path().map_err(|_| {
                    miette::miette!("Failed to convert URL {} to path", symbol.location.uri)
                })?
            }
        };
        grouped.entry(key).or_default().push(pointer);
    }

    trace!(
        "Grouped: {:?}",
        grouped
            .clone()
            .into_iter()
            .map(|it| (
                it.0,
                it.1.iter()
                    .map(|it| format!("{:?}", it))
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>()
    );

    let symbols = grouped
        .into_iter()
        .inspect(|it| {
            debug!("Grouped: {:?}", it.0);
        })
        .flat_map(|(_path, group)| {
            let sorted = group.into_iter().sorted();
            trace!("Sorted: {:?}", sorted);
            sorted.scan(false, |seen_chunk, ptr| {
                trace!("Seen chunk: {:#?} for ptr {:#?}", seen_chunk, &ptr);
                match ptr {
                    DocumentPointer::Chunk(_) => {
                        *seen_chunk = true;
                        Some(None)
                    }
                    DocumentPointer::Symbol(symbol) => {
                        if *seen_chunk {
                            *seen_chunk = false;
                            Some(Some(symbol.clone()))
                        } else {
                            Some(None)
                        }
                    }
                }
            })
        })
        .flatten()
        .inspect(|it| {
            info!("Semantic symbols after filtering: {:?}", it);
        })
        .collect::<Vec<_>>();

    Ok(symbols)
}

fn get_code_from_document(document_uri: Url, location: Option<Range>) -> Option<String> {
    let path = document_uri.to_file_path().ok();
    if let Some(path) = path {
        let text = std::fs::read_to_string(path).ok();
        if let Some(text) = text {
            let code = match location {
                Some(range) => text
                    .lines()
                    .skip(range.start.line as usize)
                    .take(range.end.line as usize - range.start.line as usize + 1)
                    .join("\n"),
                None => text,
            };
            return Some(code);
        }
    }
    None
}

pub fn get_documents_symbols(
    lsp_server: &GuardedLspServer,
    documents_uris: HashSet<Url>,
) -> impl Stream<Item = SymbolInfo> + Send {
    info!("Starting request to get document symbols");

    stream::iter(documents_uris)
        .map(move |document_uri| {
            let guarded_lsp_server = lsp_server.clone();
            async move {
                info!(
                    "Sending request to get document symbols for: {}",
                    document_uri
                );
                guarded_lsp_server
                    .send_document_symbol_request(document_uri.clone())
                    .await
                    .map(|symbols| {
                        symbols.map(|it| match it {
                            DocumentSymbolResponse::Flat(s) => {
                                let stream = stream::iter(s).map(move |symbol| {
                                    let code = get_code_from_document(
                                        document_uri.clone(),
                                        Some(symbol.location.range),
                                    );
                                    SymbolInfo {
                                        name: symbol.name,
                                        kind: format!("{:?}", symbol.kind),
                                        location: symbol.location,
                                        container_name: symbol.container_name,
                                        code,
                                    }
                                });

                                Either::Left(stream)
                            }
                            DocumentSymbolResponse::Nested(s) => {
                                let stream = stream::iter(s).map(move |symbol| {
                                    let code = get_code_from_document(
                                        document_uri.clone(),
                                        Some(symbol.range),
                                    );
                                    SymbolInfo {
                                        name: symbol.name,
                                        kind: format!("{:?}", symbol.kind),
                                        location: Location::new(document_uri.clone(), symbol.range),
                                        container_name: None,
                                        code,
                                    }
                                });
                                Either::Right(stream)
                            }
                        })
                    })
            }
        })
        .filter_map(|it| async {
            it.await
                .inspect_err(|err| {
                    error!("Error getting document symbols: {}", err);
                })
                .ok()
                .flatten()
        })
        .flat_map(|it| it)
        .boxed()
}

pub fn get_workspace_symbols(
    guarded_lsp_server: &GuardedLspServer,
    names: Vec<String>,
) -> impl Stream<Item = WorkspaceSymbolResponse> + Send {
    info!("Starting request to get workspace symbols");

    stream::iter(names)
        .map(|q| guarded_lsp_server.send_workspace_symbol_request(q))
        .filter_map(|it| async {
            it.await
                .inspect_err(|err| {
                    error!("Error getting workspace symbols: {}", err);
                })
                .ok()
                .flatten()
        })
        .boxed()
}
