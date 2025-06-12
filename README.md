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

### **Additional rules and templates**
- User can add additional rules from rules.yaml file to search result.
- Both the server description and the generated prompt can be customized using templates included in the project.
- Custom templates support dynamic insertion of variables and flexible formatting, making it possible to tailor
  responses and instructions to your specific workflow and requirements.

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

### LLM integration

```json
{
  "mcpServers": {
    "semantrix": {
      "command": "sh",
      "args": [
        "/home/i3draven/fun/Rust/semantrix/start.sh"
      ],
      "env": []
    },
  }
}
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

The answer has been shortened for convenience

```markdown
## Semantic Rules
- Always implement the `From` trait where necessary instead of writing functions or implementing the `Into` trait
## Semantic Symbols
---
**Name:** `File`
- **Kind:** `EnumMember`
- **Container:** McpSymbolKind
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/enums.rs`
    - Range: lines 35-35, columns 5-13
- **Code:**
## Fuzzy Rules
- Always implement the `From` trait where necessary instead of writing functions or implementing the `Into` trait
## Fuzzy Symbols
---
**Name:** `LspServerSubsystem`
- **Kind:** `Struct`
- **Container:** (none)
- **Location:** 
    - URI: `file:///home/i3draven/fun/Rust/semantrix/src/subsystems/lsp.rs`
    - Range: lines 87-87, columns 12-30

- **Code:**
pub struct LspServerSubsystem {
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
