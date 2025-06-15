## Slide 1: Introducing Semantrix

**Semantrix**
A Rust-based Model Context Protocol (MCP) server for intelligent code discovery, reuse, and context-aware prompt generation.

- Rule sets associated with entities in the codebase, the prompt, and the MCP server description are defined by the user.
- Combines semantic (RAG) and fuzzy (LSP) search
- Selects and applies only relevant user-defined rules for LLM prompts
- Optimizes LLM context usage for large rule sets

<div style="page-break-after: always;"></div>

---

## Slide 2: Key Capabilities

- **Semantic Search**: Finds code by meaning using machine learning embeddings (MiniLM, BGE, Nomic, etc.)
- **Fuzzy Search**: Symbol-based lookup via LSP servers (e.g., rust-analyzer)
- **Real-time Monitoring**: Watches your codebase, updates embeddings, and re-indexes on changes
- **Rule Orchestration**: Only rules relevant to found symbols are included in LLM context

<div style="page-break-after: always;"></div>

---

## Slide 3: Core Features

- **Code Reuse Focus**: Identifies existing solutions, reduces duplication
- **AI-Powered Discovery**: Fast, contextual code search with LanceDB vector storage
- **LSP Integration**: Workspace-wide, parallel symbol search
- **Prompt Templates**: Customizable with Jinja2, no rebuild needed

<div style="page-break-after: always;"></div>

---

## Slide 4: LLM \& MCP Integration

- Exposes MCP server for integration with Claude Desktop, other MCP clients, or custom apps
- Supports stdio transport for easy connectivity
- Launch script and environment variables control runtime

```json
{
  "mcpServers": {
    "semantrix": {
      "command": "sh",
      "args": ["./dist/start.sh"],
      "env": {
        "SEMANTRIX_CONFIG_PATH": "./dist/config.yml"
      }
    }
  }
}
```

<div style="page-break-after: always;"></div>

---

## Slide 5: Usage Example

**Request:**

```json
{
  "name_patterns": ["HttpHandler", "ServerSubsystem"],
  "semantic_queries": ["MCP server implementation"]
}
```

**Response:**

- Lists found symbols, locations, and code snippets
- Only relevant rules included in prompt template

<div style="page-break-after: always;"></div>

---

## Slide 6: Why Semantrix?

- Efficient LLM context management for large rule sets
- Accelerates code reuse and discovery in modern Rust projects
- Flexible, AI-powered prompt generation for developer workflows

---

**Semantrix:**
Empowering code intelligence and LLM-driven development.