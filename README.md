# Semantrix

A Model Context Protocol (MCP) server designed not just for searching, but for orchestrating code generation workflows based on discovered rules and a custom prompt template for the language model. Semantrix enables LLMs and developers to retrieve relevant code fragments using intelligent semantic (RAG) and fuzzy (LSP/rust-analyzer etc.) search, then leverages these results—together with user-defined rules and prompt templates—to guide the LLM in how to process, reuse, or transform the found code.

This approach allows the system to select and include only the rules relevant to the retrieved symbols, rather than loading the entire rule set into the model’s context. As a result, even with a large collection of rules, only the pertinent ones occupy space in the LLM context, reducing context window usage and improving efficiency

## Overview

Semantrix is a Rust-based MCP server that combines two powerful search approaches:

1. **Semantic Search**: Uses machine learning embeddings to find code based on meaning and context
2. **Fuzzy Search**: Leverages Language Server Protocol (LSP) integration for symbol-based searches

The system continuously monitors your codebase, maintains semantic embeddings of code chunks, and provides real-time search capabilities through the standardized MCP protocol.

## Key Features

### 🎯 **Code Reuse Focus**
- Designed to identify already implemented solutions
- Reduces code duplication and promotes reuse
- Helps discover existing patterns and implementations

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

## Installation & Setup

### Prerequisites

- Rust 2024 edition or later
- Linux, macOS, or Windows
- Language Server Protocol server (e.g., rust-analyzer for Rust projects or other LSP)

### Building from Source

```bash
git clone <repository-url>
cd semantrix
cargo build --release
```

### Configuration

Create a `config.yml` file in the project root. Example in project with descriptive comments.

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

### LLM integration

The launch script must be modified to match the location of your MCP server.

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

The answer has been shortened for convenience and can be customized with in project templates without rebuilding, it just jinja2 templates.

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

### Debugging

Enable debug mode in `config.yml`:
```yaml
debug: true
```

This enables:
- Verbose logging
- tokio-console integration for async debugging
- Detailed subsystem state information

### Logs

Check application logs in the configured `log_dir`:
- `semantrix.log`: Main application log

## License

This project is licensed under the terms specified in the LICENSE file.

---

**Semantrix** empowers intelligent code discovery and reuse with custom prompt generation through advanced semantic search capabilities, making it an essential tool for modern development workflows and AI-assisted programming.