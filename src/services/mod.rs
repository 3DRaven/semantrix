pub mod mcp;
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
    SymbolKind, WorkspaceSymbolResponse,
};
use miette::{IntoDiagnostic, Result, miette};
use regex::{Regex, RegexSet};
use rig::vector_store::VectorStoreIndexDyn;
use rig_fastembed::EmbeddingModel;
use rig_lancedb::LanceDbVectorIndex;
use rmcp::Error;
use serde::{Deserialize, Deserializer, Serialize};
use tera::Tera;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
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
    kinds: Vec<Regex>,
    need_code_samples: bool,
) -> Result<Vec<SymbolInfo>> {
    info!("Getting fuzzy symbols for: {:?}", possible_names);

    let symbols = get_workspace_symbols(lsp_server, possible_names)
        .await
        .flat_map(|response| match response {
            WorkspaceSymbolResponse::Flat(s) => {
                let kinds = kinds.clone();
                let stream = stream::iter(s)
                    .filter(move |symbol| {
                        let kinds = kinds.clone();
                        filter_symbols_kind(symbol.kind, kinds)
                    })
                    .map(|symbol| SymbolInfo {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind),
                        location: symbol.location,
                        container_name: symbol.container_name,
                        code: None,
                        hover: None,
                        name_position: None,
                    });

                Either::Left(stream)
            }
            WorkspaceSymbolResponse::Nested(s) => {
                let kinds = kinds.clone();
                let stream = stream::iter(s)
                    .filter(move |symbol| {
                        let kinds = kinds.clone();
                        filter_symbols_kind(symbol.kind, kinds)
                    })
                    .map(|symbol| {
                        let location = match symbol.location {
                            OneOf::Left(location) => location,
                            OneOf::Right(location) => Location::new(
                                location.uri,
                                Range::new(Position::new(0, 0), Position::new(0, 0)),
                            ),
                        };

                        SymbolInfo {
                            name: symbol.name,
                            kind: format!("{:?}", symbol.kind),
                            location,
                            container_name: symbol.container_name,
                            code: None,
                            hover: None,
                            name_position: None,
                        }
                    });
                Either::Right(stream)
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

    let symbols = update_code_and_name_position_from_document(symbols).await;

    Ok(symbols)
}

async fn filter_symbols_kind(symbol: SymbolKind, kinds: Vec<Regex>) -> bool {
    kinds
        .iter()
        .any(|kind| kind.is_match(&format!("{:?}", symbol)))
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

    let documents = get_documents_symbols(lsp_server, paths, vec![]).await;

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

async fn update_code_and_name_position_from_document(symbols: Vec<SymbolInfo>) -> Vec<SymbolInfo> {
    let groups = symbols
        .into_iter()
        .into_group_map_by(|sym| sym.location.uri.clone());

    let mut updated_symbols: Vec<SymbolInfo> = Vec::new();

    for (url, group) in groups {
        let path = url.to_file_path();
        if let Ok(path) = path {
            let file = File::open(path).await;
            if let Ok(file) = file {
                let mut lines = BufReader::new(file).lines();
                let mut index = 0;
                for mut symbol in group
                    .into_iter()
                    .sorted_by_key(|s| s.location.range.start.line)
                {
                    let regex = Regex::new(&regex::escape(&symbol.name));
                    if let Ok(regex) = regex {
                        let start_line = symbol.location.range.start.line;
                        let end_line = symbol.location.range.end.line;
                        let mut code = Vec::new();

                        trace!(
                            "Getting code and name position from document: {:?}, symbol: {:?}",
                            symbol.location.uri, symbol
                        );

                        while let Ok(line) = lines.next_line().await {
                            if index >= start_line && index <= end_line {
                                if let Some(line) = line {
                                    if symbol.name_position.is_none() {
                                        if let Some(m) = regex.find(&line) {
                                            let column = m.end() - 1;
                                            symbol.name_position =
                                                Some(Position::new(index, column as u32));
                                        }
                                    }
                                    code.push(line);
                                }
                            }
                            index += 1;
                            if index > end_line {
                                break;
                            }
                        }
                        symbol.code = Some(code.join("\n"));

                        trace!("Updated symbol: {:?}", symbol);
                        updated_symbols.push(symbol);
                    } else {
                        error!("Error creating regex for symbol: {:?}", symbol);
                    }
                }
            } else {
                updated_symbols.extend(group);
            }
        } else {
            updated_symbols.extend(group);
        }
    }

    updated_symbols
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
                        .or_else(|| {
                            Some(SymbolReferences {
                                symbol_info: symbol_info.clone(),
                                references: vec![],
                            })
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

pub async fn get_documents_symbols(
    lsp_server: &GuardedLspServer,
    documents_uris: HashSet<Url>,
    kinds: Vec<Regex>,
) -> Vec<SymbolInfo> {
    info!("Starting request to get document symbols");

    let symbols: Vec<SymbolInfo> = stream::iter(documents_uris)
        .map(move |document_uri| {
            let guarded_lsp_server = lsp_server.clone();
            let kinds = kinds.clone();
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
                                let stream = stream::iter(s)
                                    .filter(move |symbol| {
                                        let kinds = kinds.clone();
                                        filter_symbols_kind(symbol.kind, kinds)
                                    })
                                    .map(move |symbol| SymbolInfo {
                                        name: symbol.name,
                                        kind: format!("{:?}", symbol.kind),
                                        location: symbol.location,
                                        container_name: symbol.container_name,
                                        code: None,
                                        hover: None,
                                        name_position: None,
                                    });

                                Either::Left(stream)
                            }
                            DocumentSymbolResponse::Nested(s) => {
                                let stream = stream::iter(s)
                                    .filter(move |symbol| {
                                        let kinds = kinds.clone();
                                        filter_symbols_kind(symbol.kind, kinds)
                                    })
                                    .map(move |symbol| {
                                        let location =
                                            Location::new(document_uri.clone(), symbol.range);
                                        SymbolInfo {
                                            name: symbol.name,
                                            kind: format!("{:?}", symbol.kind),
                                            location,
                                            container_name: None,
                                            code: None,
                                            hover: None,
                                            name_position: Some(symbol.selection_range.end),
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
        .collect::<Vec<_>>()
        .await;

    update_code_and_name_position_from_document(symbols).await
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

pub fn get_project_files() -> Result<Vec<PathBuf>> {
    info!("Start path scanner");

    let url = Url::parse(&CONFIG.search.fuzzy.workspace_uri).into_diagnostic()?;

    if url.scheme() != "file" {
        return Err(miette!("Not a file URL: {}", url));
    }

    let path = url
        .to_file_path()
        .map_err(|_| miette!("Invalid file URL: {}", url))?;

    let positive = Glob::new(CONFIG.search.semantic.pattern.as_str()).into_diagnostic()?;

    let mut files = Vec::new();
    let walker = positive.walk(&path);

    for entry in walker
        .filter_map(|it| it.ok())
        .filter(|it| it.file_type().is_file())
    {
        info!("File found: {:?}", entry.path());
        files.push(entry.into_path());
    }

    Ok(files)
}
