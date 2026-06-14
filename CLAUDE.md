# Auger — Coding Agent

Minimal coding agent in Rust. Client-server architecture, provider-agnostic LLM access, local model first.

## Architecture

```
auger/                  # workspace root
└── provider/           # LLM provider abstraction crate
    └── src/
        ├── lib.rs      # Provider trait, ChatRequest, ChatResponse, StreamEvent, ProviderError
        └── openai.rs   # OpenAI-compatible provider impl (in progress)
```

Planned crates (not yet created):
- `agent/` — core agent loop, tool execution, system prompt
- `server/` — client-server transport layer
- `tools/` — tool definitions + shell command safety rewriter
- `ui/` — WebUI client

## Key Design Constraints

- **Local model first**: provider abstraction must work with small models over OpenAI-compatible APIs (no anthropic-specific features in core)
- **Minimal system prompt**: agent loop must be lean — avoid bloated instructions
- **Shell safety**: shell commands get classified and potentially rewritten before execution (e.g. `find -exec` → safe form, `sed -n` → `head`)
- **Client agnostic**: core agent runs as a server; UI attaches separately

## Provider Trait

Defined in `provider/src/lib.rs`:
- `Provider::chat(&self, req) -> Result<ChatResponse, ProviderError>` — one-shot
- `Provider::stream_chat(&self, req) -> Result<BoxStream<StreamEvent>, ProviderError>` — streaming

`BoxStream` and `StreamEvent` types need to be defined in `lib.rs`.

## Workspace Dependencies

Managed at workspace root `Cargo.toml` — always add new shared deps there, not per-crate.

| crate | purpose |
|---|---|
| `async-trait` | async fn in trait definitions |
| `serde` | serialization for request/response types |
| `futures` | `BoxStream`, `Stream` combinators |

## Rust Conventions

- Edition 2024
- `async-trait` for trait async methods until native async traits stabilize enough
- Prefer `thiserror` for error types (add to workspace deps when needed)
- No `unwrap()` in library code — propagate with `?`
- Keep `pub` surface minimal; expose types via `pub use` from crate root

## Build & Test

```bash
cargo build            # build all workspace members
cargo test             # run all tests
cargo clippy           # lint
cargo check            # fast type-check without codegen
```

No test infrastructure yet — add unit tests alongside each module as impl progresses.
