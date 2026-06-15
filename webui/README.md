# auger webui

Basic SvelteKit client for the auger agent-server.

## Run

Start the agent-server first (defaults to `127.0.0.1:3000`):

```bash
cargo run -p agent-server
```

Then the UI:

```bash
cd webui
pnpm install
pnpm dev
```

Open http://localhost:5173.

The dev server proxies `/v1/*` to the agent-server to avoid CORS. Override the
target with `AGENT_SERVER_URL`:

```bash
AGENT_SERVER_URL=http://host:3000 pnpm dev
```

## What it does

- **New session** — `POST /v1/sessions`, stores the returned owner token.
- **Live events** — streams `GET /v1/sessions/{id}/events` (SSE) via `fetch` +
  a `ReadableStream` reader, since `EventSource` can't send the `Bearer` token.
- **Chat** — sends turns with `POST /v1/sessions/{id}/input`; streams assistant
  `content` into a bubble, rendered as **Markdown + LaTeX** (markdown-it + KaTeX:
  `$…$`, `$$…$$`, `\(…\)`, `\[…\]`). `html: false` escapes any raw HTML in model
  output, so rendering is XSS-safe.
- **Tool approval** — on a `tool_call` event, shows Approve/Deny; replies with
  `POST /v1/sessions/{id}/approve`. The agent loop blocks until you answer.
- **Context bar + per-message stats** — the meter fills **live** while the model
  streams (rough ~4 chars/token estimate), then snaps to the server's
  authoritative `total_tokens` when the per-call `metrics` event arrives. Each
  assistant message gets a footer: completion tokens, avg tok/s, time to first
  token. The window size comes from the session-create response; the server
  reads it from the model at startup (llama.cpp `/props` → `n_ctx`), overridable
  with `CONTEXT_WINDOW` (fallback default 8192). `tok/s` is averaged over the
  whole call; `TTFT` shows prefill latency.

## Layout

```
src/
├── app.css                 # dark theme tokens
├── lib/
│   ├── api.js              # agent-server HTTP/SSE client
│   └── markdown.js         # markdown-it + KaTeX renderer
└── routes/
    ├── +layout.svelte
    └── +page.svelte        # chat UI + event handling
```

## Scripts

```bash
pnpm dev       # dev server (with /v1 proxy)
pnpm build     # production build
pnpm check     # svelte-check type check
```
