# Semantrix

An advanced Model Context Protocol (MCP) server that provides intelligent semantic and fuzzy search capabilities across project codebases. Semantrix enables LLMs and developers to efficiently discover, reuse, and analyze existing code through sophisticated AI-powered search mechanisms.

## Overview

Semantrix is a Rust-based MCP server that combines two powerful search approaches:

1. **Semantic Search**: Uses machine learning embeddings to find code based on meaning and context
2. **Fuzzy Search**: Leverages Language Server Protocol (LSP) integration for symbol-based searches

The system continuously monitors your codebase, maintains semantic embeddings of code chunks, and provides real-time search capabilities through the standardized MCP protocol.

## Key Features

### 🔍 **Dual Search Capabilities**
- **Semantic Search**: Find code using natural language descriptions or partial code constructs
- **Fuzzy Name Matching**: Locate symbols using partial or complete names with tolerance for typos

### 🤖 **AI-Powered Code Discovery**
- Machine learning embeddings for contextual code understanding
- Support for multiple embedding models (MiniLM, BGE, Nomic, etc.)
- Vector database storage using LanceDB for fast similarity searches

### 🔄 **Real-time Monitoring**
- File system watching with intelligent debouncing
- Automatic re-indexing when code changes
- Background processing pipeline for continuous updates

### 🛠 **LSP Integration**
- Supports multiple Language Server Protocol servers (rust-analyzer, etc.)
- Symbol-based searches with workspace-wide scope
- Configurable parallelism and search parameters

### 🎯 **Code Reuse Focus**
- Designed to identify already implemented solutions
- Reduces code duplication and promotes reuse
- Helps discover existing patterns and implementations

## Architecture

Semantrix uses a multi-subsystem architecture with graceful shutdown handling:

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   Watcher   │───▶│   Chunker   │───▶│   Indexer   │
│ Subsystem   │    │ Subsystem   │    │ Subsystem   │
└─────────────┘    └─────────────┘    └─────────────┘
                                             │
┌─────────────┐    ┌─────────────┐           │
│ LSP Server  │    │ MCP Server  │◀──────────┘
│ Subsystem   │───▶│ Subsystem   │
└─────────────┘    └─────────────┘
```

### Subsystems

- **Watcher**: Monitors file system changes using notify
- **Chunker**: Splits code files into semantic chunks with configurable overlap
- **Indexer**: Generates embeddings and stores them in LanceDB
- **LSP Server**: Manages Language Server Protocol communication
- **MCP Server**: Provides MCP protocol interface for external clients

## Installation & Setup

### Prerequisites

- Rust 2024 edition or later
- Linux, macOS, or Windows
- Language Server Protocol server (e.g., rust-analyzer for Rust projects)

### Building from Source

```bash
git clone <repository-url>
cd semantrix
cargo build --release
```

### Configuration

Create a `config.yml` file in the project root. Here's a basic configuration:

```yaml
# General settings
debounce_sec: 1
debug: false
shutdown_timeout: 3000
channel_size: 100
response: Prompt

# Logging
log_dir: "./logs"

# Templates
templates:
  templates_path: "./resources/templates/**/*"
  prompt: "prompt.md"
  description:
    server: "description/server.md"
    fuzzy_query: "description/fuzzy_query.md"
    semantic_query: "description/semantic_query.md"

# Search configuration
search:
  fuzzy:
    lsp_server: "rust-analyzer"
    server_args:
      - --log-file
      - rust-analyzer.log
    workspace_uri: "file:///path/to/your/project/src"
    parallelizm: 1
    server_options:
      lru:
        capacity: 44
      memoryLimit: 512
      cargo:
        loadOutDirsFromCheck: false
      workspace:
        symbol:
          search:
            kind: all_symbols
            scope: workspace
            limit: 32

  semantic:
    download_model: true
    models_dir: "./resources/models"
    model: "all-mini-lm-l6-v2-q"
    lancedb_store: "./resources/lancedb-store"
    chunk_size: 5
    overlap_size: 2
    pattern: "**/*.{rs,py,js,ts,java,c,cpp,h,hpp}"
    batch_size: 100
    search_limit: 10
    index_embeddings: false
```

### Available Embedding Models

Semantrix supports numerous pre-trained embedding models:

- **MiniLM variants**: `all-mini-lm-l6-v2`, `all-mini-lm-l6-v2-q`, `all-mini-lm-l12-v2`, `all-mini-lm-l12-v2-q`
- **BGE models**: `bge-base-en-v1.5`, `bge-large-en-v1.5`, `bge-small-en-v1.5` (+ quantized versions)
- **Nomic embeddings**: `nomic-embed-text-v1`, `nomic-embed-text-v1.5`
- **Multilingual**: `multilingual-e5-small`, `multilingual-e5-base`, `multilingual-e5-large`
- **Code-specific**: `jina-embeddings-v2-base-code`
- **GTE models**: `gte-base-en-v1.5`, `gte-large-en-v1.5`
- **MXBAI**: `mxbai-embed-large-v1`

## Usage

### Starting the Server

```bash
# Using the provided script
./start.sh

# Or directly
cargo run --release
```

### Call example

#### Request

```json
{
  "name_patterns": [
    "HttpHandler",
    "ServerSubsystem",
    "McpServer"
  ],
  "semantic_queries": [
    "MCP server implementation",
    "subsystem architecture",
    "tokio async runtime setup"
  ]
}
```

#### Response

```markdown


## Semantic Rules



- Always implement the `From` trait where necessary instead of writing functions or implementing the `Into` trait

- Never create a `new` method; instead, implement the `From` trait if it possible.

- Prefer using `inspect_err`, the `error!` macro, and error propagation instead of `map_err`

- Re-exporting is strictly prohibited in the project

- The following symbols were found: [

    SymbolInfo, 

    DocumentSymbols, 

    ChunkerSubsystem, 

    PathEvent

].
For all such structures, you must implement `#[derive(Debug)]`.


- Try to write code in a way that is easy to understand and maintain



## Semantic Symbols



---

**Name:** `File`
- **Kind:** `EnumMember`
- **Container:** McpSymbolKind
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/enums.rs`
    - Range: lines 35-35, columns 5-13

- **Code:**
```
    File = 1,
```



---

**Name:** `subsystems`
- **Kind:** `Module`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/lib.rs`
    - Range: lines 4-4, columns 1-20

- **Code:**
```
pub mod subsystems;
```



---

**Name:** `VERSION`
- **Kind:** `Constant`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/lib.rs`
    - Range: lines 48-48, columns 1-53

- **Code:**
```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
```



---

**Name:** `load_config`
- **Kind:** `Function`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/lib.rs`
    - Range: lines 157-192, columns 1-2

- **Code:**
```
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
```



---

**Name:** `init_db`
- **Kind:** `Function`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/lib.rs`
    - Range: lines 466-562, columns 1-2

- **Code:**
```
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
```



---

**Name:** `main`
- **Kind:** `Function`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/main.rs`
    - Range: lines 17-80, columns 1-2

- **Code:**
```
#[tokio::main]
async fn main() -> Result<()> {
    let _log_guard = init_logger()?;
    info!(
        "Starting server in work directory: {}",
        std::env::current_dir().into_diagnostic()?.display()
    );
   CUT---------------
```



---

**Name:** `(path_event_tx, path_event_rx)`
- **Kind:** `Variable`
- **Container:** main
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/main.rs`
    - Range: lines 25-25, columns 5-90

- **Code:**
```
    let (path_event_tx, path_event_rx) = tokio::sync::mpsc::channel(CONFIG.channel_size);
```

---

## Fuzzy Rules



- Always implement the `From` trait where necessary instead of writing functions or implementing the `Into` trait

- Never create a `new` method; instead, implement the `From` trait if it possible.

- Prefer using `inspect_err`, the `error!` macro, and error propagation instead of `map_err`

- Re-exporting is strictly prohibited in the project

- The following symbols were found: [

    LspServerSubsystem, 

    McpServerSubsystem, 

    McpServerSubsystem

].
For all such structures, you must implement `#[derive(Debug)]`.


- Try to write code in a way that is easy to understand and maintain



## Fuzzy Symbols



---

**Name:** `LspServerSubsystem`
- **Kind:** `Struct`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/subsystems/lsp.rs`
    - Range: lines 87-87, columns 12-30

- **Code:**
```
pub struct LspServerSubsystem {
```



---

### Guidance for Code Generation

- When generating code based on the discovered symbols, **always respect the rules listed under "Semantic Rules" and "Fuzzy Rules"** for each respective symbol category.
- The rules are provided as `Vec` collections and must be strictly followed during code synthesis, refactoring, or analysis.
- **Reuse already implemented entities** from the lists above whenever possible, instead of generating new ones.
- If a rule set is empty, proceed with standard code generation practices for that symbol category.

Use this symbol and rule list to analyze the project structure, search for relevant entities, and guide meaningful, context-aware code-related responses.

```

### MCP Integration

Once running, Semantrix exposes an MCP server that can be integrated with:

- **Claude Desktop**: Add as an MCP server in settings
- **Other MCP clients**: Connect via stdio transport
- **Custom applications**: Use any MCP-compatible client library

### Search Capabilities

#### Semantic Search
Query using natural language descriptions:
- "function that handles HTTP requests"
- "error handling with custom types"
- "database connection pooling"

#### Fuzzy Search
Search using symbol names or patterns:
- "HttpHandler"
- "connect_db"
- "Error"

## Configuration Options

### Core Settings

| Option | Description | Default |
|--------|-------------|---------|
| `debounce_sec` | File change debouncing time | 1 |
| `debug` | Enable debug logging and tokio-console | false |
| `shutdown_timeout` | Graceful shutdown timeout (ms) | 3000 |
| `channel_size` | Inter-subsystem channel buffer size | 100 |

### Semantic Search Settings

| Option | Description | Default |
|--------|-------------|---------|
| `model` | Embedding model to use | "all-mini-lm-l6-v2-q" |
| `chunk_size` | Lines per code chunk | 5 |
| `overlap_size` | Overlap between chunks | 2 |
| `search_limit` | Max results returned | 10 |
| `batch_size` | Embedding batch size | 100 |

### LSP Settings

| Option | Description | Default |
|--------|-------------|---------|
| `lsp_server` | LSP server executable | "rust-analyzer" |
| `workspace_uri` | Project workspace URI | Required |
| `parallelizm` | Concurrent LSP requests | 1 |

### Debugging

Enable debug mode in `config.yml`:
```yaml
debug: true
```

This enables:
- Verbose logging
- tokio-console integration for async debugging
- Detailed subsystem state information

## Troubleshooting

### Common Issues

1. **Model Download Fails**
   - Check internet connection
   - Verify `models_dir` permissions
   - Try a different model

2. **LSP Server Not Starting**
   - Verify LSP server is installed
   - Check `workspace_uri` path
   - Review server arguments

3. **High Memory Usage**
   - Reduce `chunk_size` and `batch_size`
   - Use quantized models (models ending in `-q`)
   - Enable `index_embeddings: false`

4. **Slow Indexing**
   - Increase `batch_size`
   - Use smaller embedding models
   - Narrow file `pattern` matching

### Logs

Check application logs in the configured `log_dir`:
- `semantrix.log`: Main application log
- `rust-analyzer.log`: LSP server log (if using rust-analyzer)

## License

This project is licensed under the terms specified in the LICENSE file.

---

**Semantrix** empowers intelligent code discovery and reuse through advanced semantic search capabilities, making it an essential tool for modern development workflows and AI-assisted programming.