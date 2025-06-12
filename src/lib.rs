pub mod enums;
pub mod repositories;
pub mod services;
pub mod subsystems;

use ::time::format_description;
use clap::Parser;
use config::{Config, Environment, File, FileFormat};
use convert_case::Casing;
use fastembed::ModelInfo;
use fastembed::Pooling;
use fastembed::TokenizerFiles;
use fastembed::read_file_to_bytes;
use fastembed::{EmbeddingModel, TextEmbedding, UserDefinedEmbeddingModel};
use hf_hub::Cache;
use hf_hub::api::tokio::ApiBuilder;
use hf_hub::api::tokio::ApiRepo;
use lancedb::arrow::arrow_schema::DataType;
use lancedb::{
    Connection, Table,
    index::vector::IvfPqIndexBuilder,
    table::{OptimizeAction, OptimizeOptions},
};
use miette::{IntoDiagnostic, Result};
use once_cell::sync::Lazy;
use rig_lancedb::{LanceDbVectorIndex, SearchParams};
use serde::Deserialize;
use serde_json::Value;
use std::backtrace::Backtrace;
use std::panic;
use std::path::PathBuf;
use std::sync::Arc;
use tera::Tera;
use tracing::{Level, error, info};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, time::UtcTime, writer::MakeWriterExt},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::subsystems::indexer::schema;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const LOG_DIR: &str = "logs";
pub const DEFAULT_CHUNKS_TABLE_NAME: &str = "chunks";
pub const DEFAULT_CHUNKS_ID_FIELD: &str = "id";
pub const DEFAULT_CHUNKS_PATH_FIELD: &str = "path";
pub const DEFAULT_CHUNKS_START_LINE_FIELD: &str = "start_line";
pub const DEFAULT_CHUNKS_END_LINE_FIELD: &str = "end_line";
pub const DEFAULT_CHUNKS_EMBEDDING_FIELD: &str = "embedding";

pub static ARGS: Lazy<Arc<Args>> = Lazy::new(|| {
    let args = Args::parse();
    Arc::new(args)
});

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// The path to the config file
    #[arg(short, long, value_name = "CONFIG_PATH", default_value = "config.yml")]
    pub config_path: String,
}

pub static CONFIG: Lazy<Arc<McpConfig>> = Lazy::new(|| {
    let config_path_env =
        (NAME.to_owned() + "_CONFIG_PATH").to_case(convert_case::Case::UpperSnake);
    info!(
        "Try loading config from {} environment variable",
        config_path_env
    );
    let config_path = std::env::var(config_path_env).unwrap_or_else(|_| ARGS.config_path.clone());
    let config = load_config(&config_path).expect("Failed to load config");
    Arc::new(config)
});

#[derive(Clone, Debug, Deserialize)]
pub struct SemanticConfig {
    pub download_model: bool,
    pub models_dir: PathBuf,
    pub lancedb_store: String,
    pub model: String,
    pub chunk_size: usize,
    pub overlap_size: usize,
    pub pattern: String,
    pub batch_size: usize,
    pub search_limit: usize,
    pub index_embeddings: bool,
}
#[derive(Clone, Debug, Deserialize)]

pub struct Search {
    pub semantic: SemanticConfig,
    pub fuzzy: FuzzyConfig,
}

#[derive(Clone, Debug, Deserialize)]

pub struct FuzzyConfig {
    pub lsp_server: String,
    pub server_args: Vec<String>,
    pub workspace_uri: String,
    pub server_options: Value,
    pub parallelizm: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub enum ResponseType {
    Prompt,
    Json,
}

#[derive(Clone, Debug, Deserialize)]
pub struct McpConfig {
    pub debug: bool,
    pub shutdown_timeout: u64,
    pub channel_size: usize,
    pub debounce_sec: u64,
    pub response: ResponseType,
    pub search: Search,
    pub templates: Templates,
    pub log_dir: PathBuf,
    pub rules: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Templates {
    pub templates_path: String,
    pub prompt: String,
    pub description: Description,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Description {
    pub server: String,
    pub fuzzy_query: String,
    pub semantic_query: String,
}

pub static TERA: Lazy<Tera> = Lazy::new(|| {
    Tera::new(&CONFIG.templates.templates_path)
        .inspect(|tera| {
            info!(
                "Loaded templates: {:?}",
                tera.get_template_names().collect::<Vec<_>>()
            )
        })
        .expect("Failed to create Tera instance")
});

pub fn load_config(path: &str) -> Result<McpConfig> {
    info!("Loading configuration from file: {}", path);

    let config = Config::builder()
        .add_source(
            Environment::default()
                .prefix(&NAME.to_uppercase())
                .separator("_")
                .ignore_empty(true),
        )
        .add_source(File::new(path, FileFormat::Yaml))
        .build()
        .into_diagnostic()?;

    let app_config: McpConfig = config
        .try_deserialize()
        .inspect_err(|e| error!("Failed to deserialize configuration: {}", e))
        .into_diagnostic()?;

    if app_config.search.semantic.chunk_size < 1 {
        return Err(miette::miette!(
            "chunk_size must be greater than 0, but got {}",
            app_config.search.semantic.chunk_size
        ));
    }

    if app_config.search.semantic.overlap_size > app_config.search.semantic.chunk_size - 1 {
        return Err(miette::miette!(
            "overlap_size must be less or equal to chunk_size - 1, but got {} > {}",
            app_config.search.semantic.overlap_size,
            app_config.search.semantic.chunk_size
        ));
    }

    Ok(app_config)
}

pub fn init_logger() -> Result<WorkerGuard> {
    let time_format = format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    )
    .expect("format string should be valid!");
    let timer = UtcTime::new(time_format);

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(NAME)
        .filename_suffix("log")
        .max_log_files(3)
        .build(CONFIG.log_dir.clone())
        .expect("failed to create log file appender");

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let console_filter = if !CONFIG.debug {
        Some(
            EnvFilter::new("debug")
                .add_directive("lance=off".parse().unwrap())
                .add_directive("ort=info".parse().unwrap())
                .add_directive("tokio=info".parse().unwrap())
                .add_directive("runtime=info".parse().unwrap())
                .add_directive("mcp-lsp-bridge=debug".parse().unwrap())
                .add_directive("lance_linalg=info".parse().unwrap())
                .add_directive("lance_file=info".parse().unwrap())
                .add_directive("sqlparser=info".parse().unwrap())
                .add_directive("datafusion_physical_plan=info".parse().unwrap())
                .add_directive("hyper_util=info".parse().unwrap()),
        )
    } else {
        None
    };

    let file_filter = if !CONFIG.debug {
        Some(
            EnvFilter::new("debug")
                .add_directive("lance=off".parse().unwrap())
                .add_directive("ort=info".parse().unwrap())
                .add_directive("tokio=info".parse().unwrap())
                .add_directive("runtime=info".parse().unwrap())
                .add_directive("mcp-lsp-bridge=debug".parse().unwrap())
                .add_directive("lance_linalg=info".parse().unwrap())
                .add_directive("lance_file=info".parse().unwrap())
                .add_directive("sqlparser=info".parse().unwrap())
                .add_directive("datafusion_physical_plan=info".parse().unwrap())
                .add_directive("hyper_util=info".parse().unwrap()),
        )
    } else {
        None
    };

    let stderr_layer = if CONFIG.debug {
        Some(
            fmt::layer()
                .pretty()
                .with_ansi(false)
                .with_file(true)
                .with_line_number(true)
                .with_thread_ids(true)
                .with_timer(timer.clone())
                .with_writer(std::io::stderr.with_max_level(Level::DEBUG))
                .with_filter(console_filter),
        )
    } else {
        None
    };

    let file_layer = fmt::layer()
        .pretty()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_timer(timer)
        .with_writer(non_blocking.with_max_level(Level::DEBUG))
        .with_filter(file_filter);

    let tokio_console_layer = if CONFIG.debug {
        Some(console_subscriber::spawn())
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .with(tokio_console_layer)
        .init();

    info!("Tracing initialized successfully");

    panic::set_hook(Box::new(|info| {
        error!("Panic occurred: {}", info);
        error!("Backtrace:\n{:?}", Backtrace::force_capture());
    }));

    info!("Configuration loaded successfully: {:#?}", CONFIG);

    Ok(guard)
}

pub fn model_from_str(value: &str) -> EmbeddingModel {
    match value {
        "all-mini-lm-l6-v2" => EmbeddingModel::AllMiniLML6V2,
        "all-mini-lm-l6-v2-q" => EmbeddingModel::AllMiniLML6V2Q,
        "all-mini-lm-l12-v2" => EmbeddingModel::AllMiniLML12V2,
        "all-mini-lm-l12-v2-q" => EmbeddingModel::AllMiniLML12V2Q,
        "bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
        "bge-base-en-v1.5-q" => EmbeddingModel::BGEBaseENV15Q,
        "bge-large-en-v1.5" => EmbeddingModel::BGELargeENV15,
        "bge-large-en-v1.5-q" => EmbeddingModel::BGELargeENV15Q,
        "bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "bge-small-en-v1.5-q" => EmbeddingModel::BGESmallENV15Q,
        "nomic-embed-text-v1" => EmbeddingModel::NomicEmbedTextV1,
        "nomic-embed-text-v1.5" => EmbeddingModel::NomicEmbedTextV15,
        "nomic-embed-text-v1.5-q" => EmbeddingModel::NomicEmbedTextV15Q,
        "paraphrase-mini-lm-l12-v2" => EmbeddingModel::ParaphraseMLMiniLML12V2,
        "paraphrase-mini-lm-l12-v2-q" => EmbeddingModel::ParaphraseMLMiniLML12V2Q,
        "paraphrase-mpnet-base-v2" => EmbeddingModel::ParaphraseMLMpnetBaseV2,
        "bge-small-zh-v1.5" => EmbeddingModel::BGESmallZHV15,
        "multilingual-e5-small" => EmbeddingModel::MultilingualE5Small,
        "multilingual-e5-base" => EmbeddingModel::MultilingualE5Base,
        "multilingual-e5-large" => EmbeddingModel::MultilingualE5Large,
        "mxbai-embed-large-v1" => EmbeddingModel::MxbaiEmbedLargeV1,
        "mxbai-embed-large-v1-q" => EmbeddingModel::MxbaiEmbedLargeV1Q,
        "gte-base-en-v1.5" => EmbeddingModel::GTEBaseENV15,
        "gte-base-en-v1.5-q" => EmbeddingModel::GTEBaseENV15Q,
        "gte-large-en-v1.5" => EmbeddingModel::GTELargeENV15,
        "gte-large-en-v1.5-q" => EmbeddingModel::GTELargeENV15Q,
        "clip-vit-b-32-text" => EmbeddingModel::ClipVitB32,
        "jina-embeddings-v2-base-code" => EmbeddingModel::JinaEmbeddingsV2BaseCode,
        _ => EmbeddingModel::AllMiniLML6V2,
    }
}
pub fn retrieve_model(model: EmbeddingModel, cache_dir: PathBuf) -> Result<ApiRepo> {
    let cache = Cache::new(cache_dir);
    let api = ApiBuilder::from_cache(cache)
        .with_progress(false)
        .build()
        .into_diagnostic()?;

    let model_id = model.to_string();
    info!("Retrieving model from Hugging Face: {}", model_id);
    let repo = api.model(model_id);
    Ok(repo)
}

pub async fn get_or_create_table(db: &Connection, ndims: usize) -> Result<Table> {
    let table = if db
        .table_names()
        .execute()
        .await
        .into_diagnostic()?
        .contains(&DEFAULT_CHUNKS_TABLE_NAME.to_string())
    {
        let table = db
            .open_table(DEFAULT_CHUNKS_TABLE_NAME)
            .execute()
            .await
            .into_diagnostic()?;
        let current_schema = table.schema().await.into_diagnostic()?;
        info!("Schema: {:?}", current_schema);
        let embedding_field = current_schema
            .field_with_name(DEFAULT_CHUNKS_EMBEDDING_FIELD)
            .into_diagnostic()?;
        let new_table = if let DataType::FixedSizeList(_, dims) = embedding_field.data_type() {
            if *dims != ndims as i32 {
                info!(
                    "Embedding field data type size is not equal to ndims of current model, dropping table: {} != {}",
                    *dims, ndims
                );
                db.drop_table(DEFAULT_CHUNKS_TABLE_NAME)
                    .await
                    .into_diagnostic()?;
                let new_schema = schema(ndims);
                info!("Creating new table with schema: {:?}", new_schema);
                Some(
                    db.create_empty_table(DEFAULT_CHUNKS_TABLE_NAME, Arc::new(new_schema))
                        .execute()
                        .await
                        .into_diagnostic()?,
                )
            } else {
                None
            }
        } else {
            return Err(miette::miette!(
                "Embedding field is not a FixedSizeList: {:?}",
                embedding_field.data_type()
            ));
        };
        new_table.unwrap_or(table)
    } else {
        db.create_empty_table(DEFAULT_CHUNKS_TABLE_NAME, Arc::new(schema(ndims)))
            .execute()
            .await
            .into_diagnostic()?
    };

    Ok(table)
}

pub async fn get_or_download_model(
    model: EmbeddingModel,
    model_info: &ModelInfo<EmbeddingModel>,
) -> Result<(PathBuf, TokenizerFiles)> {
    let model = if CONFIG.search.semantic.download_model {
        info!(
            "Downloading model from Hugging Face to {:?}",
            CONFIG.search.semantic.models_dir
        );
        let model_repo =
            retrieve_model(model.to_owned(), CONFIG.search.semantic.models_dir.clone())?;
        info!("Model repo: {:?}", model_repo);
        let model_path = model_repo
            .get(&model_info.model_file)
            .await
            .into_diagnostic()?;
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read_file_to_bytes(
                &model_repo.get("tokenizer.json").await.into_diagnostic()?,
            )
            .map_err(|e| miette::miette!("Failed to read tokenizer.json: {}", e))?,
            config_file: read_file_to_bytes(
                &model_repo.get("config.json").await.into_diagnostic()?,
            )
            .map_err(|e| miette::miette!("Failed to read config.json: {}", e))?,
            special_tokens_map_file: read_file_to_bytes(
                &model_repo
                    .get("special_tokens_map.json")
                    .await
                    .into_diagnostic()?,
            )
            .map_err(|e| miette::miette!("Failed to read special_tokens_map.json: {}", e))?,
            tokenizer_config_file: read_file_to_bytes(
                &model_repo
                    .get("tokenizer_config.json")
                    .await
                    .into_diagnostic()?,
            )
            .map_err(|e| miette::miette!("Failed to read tokenizer_config.json: {}", e))?,
        };
        (model_path, tokenizer_files)
    } else {
        info!(
            "Loading model from local directory {:?}",
            CONFIG.search.semantic.models_dir
        );
        let model_dir = CONFIG.search.semantic.models_dir.join(model.to_string());
        info!("Model directory: {:?}", model_dir);
        let model_path = model_dir.join(&model_info.model_file);
        info!("Model path: {:?}", model_path);

        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read_file_to_bytes(&model_dir.join("tokenizer.json"))
                .map_err(|e| miette::miette!("Failed to read tokenizer.json: {}", e))?,
            config_file: read_file_to_bytes(&model_dir.join("config.json"))
                .map_err(|e| miette::miette!("Failed to read config.json: {}", e))?,
            special_tokens_map_file: read_file_to_bytes(&model_dir.join("special_tokens_map.json"))
                .map_err(|e| miette::miette!("Failed to read special_tokens_map.json: {}", e))?,
            tokenizer_config_file: read_file_to_bytes(&model_dir.join("tokenizer_config.json"))
                .map_err(|e| miette::miette!("Failed to read tokenizer_config.json: {}", e))?,
        };

        (model_path, tokenizer_files)
    };

    Ok(model)
}

pub async fn init_db() -> Result<(
    usize,
    Table,
    rig_fastembed::EmbeddingModel,
    Arc<LanceDbVectorIndex<rig_fastembed::EmbeddingModel>>,
)> {
    let db: Connection = lancedb::connect(&CONFIG.search.semantic.lancedb_store)
        .execute()
        .await
        .into_diagnostic()?;

    let model = model_from_str(&CONFIG.search.semantic.model);
    let model_info = TextEmbedding::get_model_info(&model).map_err(|e| {
        miette::miette!(
            "Failed to get model info for model: {:?}, error: {}",
            model,
            e
        )
    })?;
    info!("Model info: {:?}", model_info);
    let (model_path, tokenizer_files) = get_or_download_model(model.clone(), model_info).await?;
    info!("Reading model.onnx file from {:?}", model_path);
    let onnx_file = read_file_to_bytes(&model_path).expect("Could not read model.onnx file");
    info!("Creating embedding model");
    let user_defined_model =
        UserDefinedEmbeddingModel::new(onnx_file, tokenizer_files).with_pooling(Pooling::Mean);

    let ndims = model_info.dim;

    let embedding_model =
        rig_fastembed::EmbeddingModel::new_from_user_defined(user_defined_model, ndims, model_info);

    let table: Table = get_or_create_table(&db, ndims).await?;

    if table
        .index_stats(DEFAULT_CHUNKS_PATH_FIELD)
        .await
        .into_diagnostic()?
        .is_none()
    {
        table
            .create_index(&[DEFAULT_CHUNKS_PATH_FIELD], lancedb::index::Index::Auto)
            .execute()
            .await
            .into_diagnostic()?;
    }

    if CONFIG.search.semantic.index_embeddings
        && table
            .index_stats(DEFAULT_CHUNKS_EMBEDDING_FIELD)
            .await
            .into_diagnostic()?
            .is_none()
    {
        // See [LanceDB indexing](https://lancedb.github.io/lancedb/concepts/index_ivfpq/#product-quantization) for more information
        table
            .create_index(
                &[DEFAULT_CHUNKS_EMBEDDING_FIELD],
                lancedb::index::Index::IvfPq(IvfPqIndexBuilder::default()),
            )
            .execute()
            .await
            .into_diagnostic()?;
    } else if !CONFIG.search.semantic.index_embeddings
        && table
            .index_stats(DEFAULT_CHUNKS_EMBEDDING_FIELD)
            .await
            .into_diagnostic()?
            .is_some()
    {
        table
            .drop_index(DEFAULT_CHUNKS_EMBEDDING_FIELD)
            .await
            .into_diagnostic()?;
        table
            .optimize(OptimizeAction::Index(OptimizeOptions::default()))
            .await
            .into_diagnostic()?;
    }

    info!("Table: {:?}", table.schema().await.into_diagnostic()?);

    let search_params = SearchParams::default();

    let vector_store = Arc::new(
        LanceDbVectorIndex::new(
            table.clone(),
            embedding_model.clone(),
            DEFAULT_CHUNKS_ID_FIELD,
            search_params,
        )
        .await
        .into_diagnostic()?,
    );

    Ok((ndims, table, embedding_model, vector_store))
}
