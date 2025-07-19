pub mod mcp;
use line_column::line_column;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use futures::{Stream, StreamExt, TryStreamExt, future::Either, stream};
use itertools::Itertools;
use lsp_types::{
    DocumentSymbolResponse, Hover, HoverContents, Location, MarkedString, OneOf, Position, Range,
    WorkspaceSymbolResponse,
};
use miette::{IntoDiagnostic, Result};
use regex::{Regex, RegexSet};
use rig::vector_store::VectorStoreIndexDyn;
use rig_fastembed::EmbeddingModel;
use rig_lancedb::LanceDbVectorIndex;
use rmcp::Error;
use serde::{Deserialize, Deserializer, Serialize};
use tera::Tera;
use tracing::{debug, error, info, trace};
use url::Url;
use wax::{Glob, Pattern};

use crate::{
    CONFIG,
    subsystems::{
        chunker::{ChunkId, DocumentPointer},
        lsp::GuardedLspServer,
    },
};

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub location: Location,
    pub container_name: Option<String>,
    pub code: Option<String>,
    pub hover: Option<String>,
    pub name_position: Option<Position>,
}

impl SymbolInfo {
    pub fn path(&self) -> Result<PathBuf> {
        self.location
            .uri
            .to_file_path()
            .map_err(|_| miette::miette!("Failed to convert URL {} to path", self.location.uri))
    }

    pub fn set_hover(&mut self, hover: Hover) {
        self.hover = Some(match &hover.contents {
            HoverContents::Scalar(s) => match s {
                MarkedString::String(s) => s.to_owned(),
                MarkedString::LanguageString(s) => s.value.to_owned(),
            },
            HoverContents::Array(s) => s
                .iter()
                .map(|s| match s {
                    MarkedString::String(s) => s.to_owned(),
                    MarkedString::LanguageString(s) => s.value.to_owned(),
                })
                .join("\n"),
            HoverContents::Markup(s) => s.value.to_owned(),
        })
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

#[derive(Debug)]
pub struct DocumentSymbols {
    pub path: Url,
    pub symbols: DocumentSymbolResponse,
}

async fn get_fuzzy_symbols(
    lsp_server: &GuardedLspServer,
    possible_names: Vec<String>,
    kinds: Option<Vec<Regex>>,
    need_code_samples: bool,
) -> Result<Vec<SymbolInfo>> {
    info!("Getting fuzzy symbols for: {:?}", possible_names);

    let symbols = get_workspace_symbols(lsp_server, possible_names)
        .await
        .flat_map(|response| match response {
            WorkspaceSymbolResponse::Flat(s) => {
                let stream = stream::iter(s).map(|symbol| {
                    let location = symbol.location.clone();
                    let code = if need_code_samples {
                        get_code_from_document(location.uri, Some(location.range))
                    } else {
                        None
                    };
                    let name_position = code
                        .as_ref()
                        .and_then(|code| get_name_position(&symbol.name, code, &symbol.location));
                    SymbolInfo {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind),
                        location: symbol.location,
                        container_name: symbol.container_name,
                        code,
                        hover: None,
                        name_position,
                    }
                });

                Either::Left(stream)
            }
            WorkspaceSymbolResponse::Nested(s) => {
                let stream = stream::iter(s).map(|symbol| {
                    let code = match symbol.location.clone() {
                        OneOf::Left(location) => {
                            if need_code_samples {
                                get_code_from_document(location.uri, Some(location.range))
                            } else {
                                None
                            }
                        }
                        OneOf::Right(location) => {
                            if need_code_samples {
                                get_code_from_document(location.uri, None)
                            } else {
                                None
                            }
                        }
                    };

                    let location = match symbol.location {
                        OneOf::Left(location) => location,
                        OneOf::Right(location) => Location::new(
                            location.uri,
                            Range::new(Position::new(0, 0), Position::new(0, 0)),
                        ),
                    };

                    let name_position = code
                        .as_ref()
                        .and_then(|code| get_name_position(&symbol.name, code, &location));

                    SymbolInfo {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind),
                        location,
                        container_name: symbol.container_name,
                        code,
                        hover: None,
                        name_position,
                    }
                });
                Either::Right(stream)
            }
        })
        .filter_map(|symbol| async {
            if let Some(kinds) = &kinds {
                if kinds
                    .iter()
                    .any(|kind| kind.is_match(&format!("{:?}", symbol.kind)))
                {
                    Some(symbol)
                } else {
                    debug!("Not matched kind: {:?}", symbol.kind);
                    None
                }
            } else {
                Some(symbol)
            }
        })
        .then(|mut it| async {
            if need_code_samples {
                let hover = get_hover(lsp_server, &it).await;
                if let Some(hover) = hover {
                    it.set_hover(hover);
                }
            }
            it
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

    let documents = get_documents_symbols(lsp_server, paths, true)
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

    let symbols = stream::iter(grouped)
        .inspect(|it| {
            debug!("Grouped: {:?}", it.0);
        })
        .flat_map(|(_path, group)| {
            let sorted = group.into_iter().sorted();
            trace!("Sorted: {:?}", sorted);
            let scaned = sorted.scan(false, |seen_chunk, ptr| {
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
            });

            stream::iter(scaned)
        })
        .filter_map(|it| async {
            if let Some(mut it) = it {
                let hover = get_hover(lsp_server, &it).await;
                if let Some(hover) = hover {
                    it.set_hover(hover);
                }
                Some(it)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .await;

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

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolPlaceTo {
    pub symbol_info: SymbolInfo,
    pub place_to: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolReferences {
    pub symbol_info: SymbolInfo,
    pub references: Vec<Location>,
}

pub fn get_symbols_references(
    lsp_server: &GuardedLspServer,
    symbol_infos: Vec<SymbolInfo>,
) -> impl Stream<Item = SymbolReferences> + Send {
    info!("Starting request to get symbols references");

    stream::iter(symbol_infos)
        .filter_map(|it| async {
            if CONFIG
                .placer
                .final_symbol_kinds
                .iter()
                .any(|pattern| pattern.is_match(&it.kind))
            {
                Some(it)
            } else {
                None
            }
        })
        .map(move |symbol_info| {
            let guarded_lsp_server = lsp_server.clone();
            async move {
                info!(
                    "Sending request to get symbols references for: {:?}",
                    symbol_info
                );
                guarded_lsp_server
                    .send_references_request(
                        symbol_info.location.uri.clone(),
                        symbol_info
                            .name_position
                            .unwrap_or(symbol_info.location.range.start),
                    )
                    .await
                    .map(|it| {
                        it.map(|it| SymbolReferences {
                            symbol_info: symbol_info.clone(),
                            references: it,
                        })
                    })
            }
        })
        .filter_map(|it| async {
            it.await
                .inspect_err(|err| {
                    error!("Error getting symbols references: {}", err);
                })
                .ok()
                .flatten()
        })
        .boxed()
}

pub fn get_documents_symbols(
    lsp_server: &GuardedLspServer,
    documents_uris: HashSet<Url>,
    need_code_samples: bool,
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
                                    let code = if need_code_samples {
                                        get_code_from_document(
                                            document_uri.clone(),
                                            Some(symbol.location.range),
                                        )
                                    } else {
                                        None
                                    };
                                    let name_position = code.as_ref().and_then(|code| {
                                        get_name_position(&symbol.name, code, &symbol.location)
                                    });

                                    SymbolInfo {
                                        name: symbol.name,
                                        kind: format!("{:?}", symbol.kind),
                                        location: symbol.location,
                                        container_name: symbol.container_name,
                                        code,
                                        hover: None,
                                        name_position,
                                    }
                                });

                                Either::Left(stream)
                            }
                            DocumentSymbolResponse::Nested(s) => {
                                let stream = stream::iter(s).map(move |symbol| {
                                    let code = if need_code_samples {
                                        get_code_from_document(
                                            document_uri.clone(),
                                            Some(symbol.range),
                                        )
                                    } else {
                                        None
                                    };
                                    let location =
                                        Location::new(document_uri.clone(), symbol.range);
                                    let name_position = code.as_ref().and_then(|code| {
                                        get_name_position(&symbol.name, code, &location)
                                    });
                                    SymbolInfo {
                                        name: symbol.name,
                                        kind: format!("{:?}", symbol.kind),
                                        location,
                                        container_name: None,
                                        code,
                                        hover: None,
                                        name_position,
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

fn get_name_position(name: &str, code: &str, location: &Location) -> Option<Position> {
    if let Ok(re) = Regex::new(format!("(?m){}", name).as_str()) {
        if let Some(m) = re.find(code) {
            trace!(
                "Match found for: {} in {} from {} to {}",
                name,
                code,
                m.start(),
                m.end()
            );
            //end because it is precisely than start, rust-analyzer some time return wrong start position
            //for name of symbol (as example `fn main` instead of `main` as function name), so when
            //we try to find them in code, we need to use end
            //local_line is 1 based index
            //col is 1 based index
            let (local_line, col) = line_column(code, m.end());
            let line = location.range.start.line + local_line - 1;
            info!(
                r#" 
                    Name: {}
                    Symbol positions:
                        Line in file: {}
                        Line in code fragment (1 based): {}
                        Position in code fragment (1 based): {}
                        Symbol code start line: {}
                        Symbol code start position: {}
                        Code fragment start position: {}
                        Code fragment end position: {}"#,
                name,
                line,
                local_line,
                col + 1,
                location.range.start.line,
                location.range.start.character,
                m.start(),
                m.end()
            );
            Some(Position::new(line, col - 1))
        } else {
            trace!("No match found for: {} in {}", name, code);
            None
        }
    } else {
        trace!("Regex error for: {}", name);
        None
    }
}

async fn get_hover(lsp_server: &GuardedLspServer, symbol: &SymbolInfo) -> Option<Hover> {
    if let Some(position) = symbol.name_position {
        let hover = lsp_server
            .send_hover_request(symbol.location.uri.clone(), position)
            .await
            .ok()
            .flatten();
        if let Some(hover) = hover {
            info!("Hover: {:?}", hover);
            return Some(hover);
        } else {
            trace!("No hover found for: {:?}", symbol);
        }
    }
    None
}

pub async fn get_workspace_symbols(
    guarded_lsp_server: &GuardedLspServer,
    names: Vec<String>,
) -> impl Stream<Item = WorkspaceSymbolResponse> + Send {
    info!("Starting request to get workspace symbols");

    if names.is_empty() {
        if let Ok(response) = guarded_lsp_server
            .send_workspace_symbol_request("".to_string())
            .await
        {
            match response {
                Some(response) => stream::once(async { response }).boxed(),
                None => stream::empty().boxed(),
            }
        } else {
            stream::empty().boxed()
        }
    } else {
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
}

fn path_distance(a: &Path, b: &Path) -> usize {
    let a: Vec<_> = a.components().collect();
    let b: Vec<_> = b.components().collect();

    let common_len = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    (a.len() - common_len) + (b.len() - common_len)
}

//TODO: no graphs for POC
pub fn find_max_distance_paths(candidates: &[PathBuf], usages: &[PathBuf]) -> Vec<PathBuf> {
    candidates
        .iter()
        .max_set_by_key(|candidate| {
            usages
                .iter()
                .map(|usage| path_distance(candidate.as_path(), usage.as_path()))
                .sum::<usize>()
        })
        .iter()
        .map(|it| (*it).clone())
        .collect::<Vec<_>>()
}

//TODO: no graphs for POC
pub fn find_min_distance_paths(candidates: &[PathBuf], usages: &[PathBuf]) -> Vec<PathBuf> {
    candidates
        .iter()
        .min_set_by_key(|candidate| {
            usages
                .iter()
                .map(|usage| path_distance(candidate.as_path(), usage.as_path()))
                .sum::<usize>()
        })
        .iter()
        .map(|it| (*it).clone())
        .collect::<Vec<_>>()
}

pub fn most_common_parent(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();

    for path in paths {
        if let Some(parent) = path.parent() {
            let parent_buf = parent.to_path_buf();
            *counts.entry(parent_buf).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(path, _)| path)
}
