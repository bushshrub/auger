# WEBUI-CHANGE.md — Port the new auger webui to Svelte + the new agent-server API

**Audience:** implementing agent. This document is self-contained; you should not need
outside context beyond the two source trees listed below.

**Goal:** Replace the current SvelteKit webui (this directory) with the UI design and
behavior of the new React/Next.js webui delivered as a zip, **ported to Svelte 5 /
SvelteKit** (we are NOT adopting Next.js), and **rewired to the new agent-server HTTP
API spec** (section 4). The zip's client code targets the *old* API — do not copy its
API layer verbatim; section 5 lists every mismatch.

Source material:

- Reference UI (React, from the zip): extracted at `/tmp/auger-webui/`.
  If missing, re-extract: `unzip /Users/robert/Downloads/auger-webui.zip -d /tmp/auger-webui`.
- Current Svelte webui: `webui/` (this directory) — being replaced, but several
  pieces are reusable (section 7.6, 7.7).

Out of scope for you: the Rust `agent-server` rewrite itself. The server will be
brought up to the same spec separately. You build the frontend against the spec, and
update the local mock server (section 8) so the UI is fully exercisable before the
Rust server lands.

---

## 1. What the zip contains (inventory)

A Next.js 16 / React 19 / Tailwind v4 app, dark "terminal" theme (Claude-Code-ish warm
palette), single-page session UI.

| File | Purpose |
|---|---|
| `app/layout.tsx` | Root layout: Geist + Geist Mono fonts, dark color-scheme, metadata. |
| `app/globals.css` | Tailwind v4 theme. All design tokens as CSS custom properties (oklch), shadcn-style names (`--background`, `--primary`, `--sidebar`, …), plus `.auger-caret` blinking-cursor and `.auger-scroll` thin-scrollbar utilities. **Port this nearly verbatim** — it is the visual identity. |
| `app/page.tsx` | Main page: sidebar + active `SessionView`, mobile header with session `<select>`, ASCII-art welcome screen. Uses SWR to poll `listSessions()` every 10 s (**changes** — no list endpoint anymore, see 5.6). |
| `components/auger/session-sidebar.tsx` | Desktop sidebar: logo, model `<select>` (hardcoded model list), "New session" button, session list (short id + model + created time), footer link. |
| `components/auger/session-view.tsx` | Per-session view: status bar (connection dot, model, short id, total token count, context-percentage bar), auto-scrolling transcript (pin-to-bottom unless user scrolled up), error banner, composer. |
| `components/auger/messages.tsx` | `UserMessage` (`>`-prefixed monospace) and `AssistantMessage` (collapsible "reasoning" section with brain icon, markdown content, blinking caret while streaming, "thinking" placeholder). |
| `components/auger/tool-call-card.tsx` | Collapsible tool-call card: icon per tool name, one-line arg summary, status badge (`needs approval` / `running` / `done` / `auto` / `denied` / `error`), expanded view shows pretty-printed args (or a diff for `edit`), output `<pre>`, error `<pre>`, Approve/Deny buttons when pending. Auto-expands when pending approval. |
| `components/auger/diff-viewer.tsx` | Naive old/new line diff table (all old lines red `-`, all new lines green `+`). We will use the existing, better Svelte DiffViewer instead (7.6). |
| `components/auger/composer.tsx` | Textarea composer: Enter sends (Shift+Enter newline, IME-safe), send button with spinner, hint footer ("enter to send…", "shell / edit / write require approval"). |
| `components/auger/markdown.tsx` | react-markdown + GFM with styled elements. Replaced by existing markdown-it setup (7.7). |
| `lib/auger/types.ts` | Protocol + UI types. Protocol types target the **old** wire format (externally-tagged serde enums, owner/viewer tokens, `input_tokens`/`output_tokens` usage) — all superseded by section 4. The **UI-side** types (`UiItem`, `UiToolCall`, `ToolCallStatus`, `APPROVAL_REQUIRED_TOOLS = {shell, edit, write}`) are good; keep them. |
| `lib/auger/api.ts` | Fetch client for the old API. Superseded — rewrite per section 6.1. |
| `lib/auger/use-auger-session.ts` | The core session state machine (React `useReducer`). **The logic is the valuable part** — port it to a Svelte 5 runes class, with the event names remapped (section 6.2). |
| `lib/auger/mock-server.ts`, `app/v1/**/route.ts` | In-process mock of the old API for the Next.js dev server. Do not port; write a new SvelteKit mock implementing the *new* spec (section 8). |
| `components/ui/button.tsx`, `lib/utils.ts` (`cn`) | shadcn scaffolding. Not needed; the auger components use plain class strings almost everywhere. A tiny `cn`-like helper or Svelte `class:` directives suffice. |
| `next.config.mjs`, `postcss.config.mjs`, `components.json`, `tsconfig.json`, `pnpm-lock.yaml` | Next.js build config — discard. |

---

## 2. Target stack

Keep the SvelteKit project in `webui/` and rework it:

- **Svelte 5 (runes) + SvelteKit** — already in `package.json`.
- **Add Tailwind CSS v4** via `@tailwindcss/vite` (the zip's UI is 100% Tailwind
  utility classes; translating them all to handwritten CSS would be error-prone).
  Setup: `pnpm add -D tailwindcss @tailwindcss/vite`, add the plugin to
  `vite.config.js`, and make `src/app.css` start with `@import 'tailwindcss';`
  followed by the ported theme from `app/globals.css`.
  - Drop the zip's `@import 'tw-animate-css'` and `@import 'shadcn/tailwind.css'`
    lines; the only animation used is `animate-spin` (built into Tailwind) and the
    custom `auger-blink` keyframes (already in globals.css).
  - Keep the `@theme inline { --color-* … --font-* --radius-* }` block and the
    `:root { … }` token block verbatim.
  - Fonts: no next/font. Use system stacks or self-host; simplest faithful option:
    `--font-sans: ui-sans-serif, system-ui, …` and `--font-mono: ui-monospace,
    SFMono-Regular, Menlo, monospace`, or add Geist via fontsource
    (`@fontsource-variable/geist`, `@fontsource-variable/geist-mono`) if trivial.
- **Icons:** `pnpm add lucide-svelte` — same icon names as `lucide-react`
  (`TerminalSquare`, `Loader2`, `Plus`, `ChevronDown`, `ChevronRight`, `Brain`,
  `Check`, `X`, `CircleAlert`, `ShieldCheck`, `Terminal`, `FilePen`, `FilePlus`,
  `FileText`, `Search`, `FolderTree`, `ListTodo`, `Globe`, `Download`,
  `CornerDownLeft`).
- **Markdown:** reuse the existing `src/lib/markdown.js` (markdown-it + katex)
  instead of react-markdown. See 7.7.
- **Diff:** reuse the existing `src/lib/DiffViewer.svelte` (@git-diff-view/svelte)
  instead of the zip's naive diff. See 7.6.
- **No SWR** — no list endpoint exists anymore; session list is client-side
  (localStorage), section 5.6.

### Files to DELETE from the current webui

- `src/routes/sessions/[id]/+page.svelte` (old per-session route; new UI is single-page)
- `src/lib/mock-store.js` and all of `src/routes/v1/**` (old-shape mock — replaced by
  new-spec mock, section 8)
- `src/lib/api.js` (old client — replaced)
- The old `+page.svelte` / `+layout.svelte` contents (replaced by the ported UI)

### Target file map

```
webui/src/
  app.css                            # tailwind import + ported theme tokens + caret/scroll utils
  app.html                           # keep; ensure <html> gets dark color-scheme
  routes/
    +layout.svelte                   # imports app.css, minimal shell
    +page.svelte                     # port of app/page.tsx (sidebar + view + welcome)
    v1/                              # NEW mock server, spec-shaped (section 8)
      sessions/+server.js
      sessions/[id]/+server.js                     # GET info, DELETE
      sessions/[id]/messages/+server.js
      sessions/[id]/tool_calls/[call_id]/+server.js
      sessions/[id]/interrupt/+server.js
      sessions/[id]/events/+server.js
      sessions/[id]/snapshot/+server.js
  lib/
    api.js                           # NEW client, section 6.1
    session.svelte.js                # NEW runes state machine, section 6.2 (port of use-auger-session)
    session-list.svelte.js           # NEW localStorage session registry, section 5.6
    mock/engine.js                   # NEW mock engine backing routes/v1 (section 8)
    markdown.js                      # existing, keep
    DiffViewer.svelte                # existing, keep
    components/
      SessionSidebar.svelte
      SessionView.svelte
      Messages.svelte                # or UserMessage.svelte + AssistantMessage.svelte
      ToolCallCard.svelte
      Composer.svelte
```

---

## 3. UI behavior to preserve exactly (from the zip)

1. **Transcript pinning:** auto-scroll to bottom on new items *only if* the user is
   within 80 px of the bottom; scrolling up unpins (see `session-view.tsx:70-77`).
2. **Streaming assistant bubble:** deltas append to the last assistant item while
   `streaming: true`; a `tool_call_request`/`tool_call_auto_approved` closes the
   current bubble (`streaming: false`) so post-tool content starts a new one;
   `turn_done` finalizes it and records usage/stop reason.
3. **Tool card lifecycle:** `pending_approval → running → done|denied|error`;
   auto-approved tools appear directly as `running` with an `auto` badge on
   completion. Cards auto-open when pending approval; edit-tool args with
   `old_string`/`new_string` render as a diff.
4. **Approval gating is client-side cosmetic:** `APPROVAL_REQUIRED_TOOLS = new
   Set(['shell','edit','write'])` decides whether a `tool_call_request` renders as
   pending or running. The server's `tool_call_auto_approved` event upgrades a card
   to running/auto regardless.
5. **Composer:** Enter submits unless Shift held or IME composing (`isComposing` /
   keyCode 229 guard); disabled while busy/sending; refocus after send.
6. **Status bar:** green/red connection dot, model name, 8-char session id, total
   token count, context-usage bar (% of context window, red above 80%).
   - Context window: the new API doesn't return one (5.7). Use a
     `PUBLIC_CONTEXT_WINDOW`-style constant/env default (e.g. 113072) and label the
     bar as approximate.
7. **Welcome screen:** ASCII "auger" banner, blurb, "Start a session" button, three
   feature tiles. Update the tile copy `['sessions', 'owner + viewer tokens per
   session']` — tokens no longer exist; say e.g. `'create, resume, and delete
   sessions'`.
8. **Mobile:** sidebar hidden below `md`; header shows a session `<select>` and a
   New button.
9. **Empty session card:** "session ready — {model}" card with usage hints.

New UI affordances required by the new API (not in the zip):

10. **Delete session** — small ✕/trash on sidebar rows → `DELETE /sessions/{id}`,
    remove from local registry, clear active view if it was active.
11. **Interrupt** — a "Stop" button in the composer area, visible while `busy` →
    `POST /sessions/{id}/interrupt`. The server returns **501 not_implemented** until
    core support lands: on 501, show a non-fatal notice ("interrupt not supported by
    this server yet"); on 202, just wait — cancellation shows up on the event stream
    later (`status_changed` → `aborted`; not emitted yet, see 4.4).
12. **Resync handling** — on a `resync_required` event, refetch `/snapshot`, rebuild
    the transcript from it, and keep the existing stream (section 6.3).

---

## 4. The new agent-server API (the spec — normative)

All requests/responses are JSON. Base URL is configurable (section 6.1). Routes have
**no `/v1` prefix** on the real server; the dev mock mounts them under `/v1` and the
client's base-URL default points there.

### 4.1 Routes

| Method & path | Purpose |
|---|---|
| `POST /sessions` | Create session. Body `{ "model": "..." }` (optional). → **201** `{ "session_id": "uuid", "model": "gemma4-12b", "created_at": 1781475209 }` (unix seconds). |
| `GET /sessions/{id}` | Session info + live state. → **200** `{ "session_id", "model", "created_at", "status": "unknown", "pending_tool_calls": [] }`. `status` is a `SessionStatus` (4.4); until core support lands the server always reports `"unknown"` and `[]`. |
| `DELETE /sessions/{id}` | Terminate + remove. → **204** empty body, or 404. |
| `POST /sessions/{id}/messages` | Send user message. Body `{ "message": "text" }`. → **202** `{ "accepted": true }`, or 404/410. |
| `POST /sessions/{id}/tool_calls/{call_id}` | Approve/deny a pending tool call. Body `{ "decision": "approve" \| "deny", "message": "optional user note" }`. → **202** / 404 / 410. An unknown `call_id` is still 202 — rejection surfaces on the event stream. |
| `POST /sessions/{id}/interrupt` | Cancel in-flight turn. Empty body. → **202** `{ "accepted": true }` / 404 / 410 / **501** (`not_implemented`) until core support exists. Fire-and-forget: the outcome is observed on the event stream, never in this response. |
| `GET /sessions/{id}/events` | SSE stream of Envelopes (4.2). Accepts `?token=` query param for auth (browser EventSource can't set headers); with no auth configured the param is ignored. `: keepalive` comment every 15 s. |
| `GET /sessions/{id}/snapshot` | → **200** `{ "as_of_seq": null, "messages": [ ... ] }` (4.5). |

There is **no session list endpoint**, no replay/`Last-Event-ID`, no models endpoint.
Do not build the UI on any of those.

### 4.2 SSE envelope

Every SSE event: `event:` field = the envelope `type`; `data:` = the full envelope
JSON. All enum values are **snake_case strings**.

```json
{ "seq": null, "type": "content_delta", "data": { "delta": "..." } }
```

`seq` is always `null` for now (reserved for future sequence numbers — always parse
it, never require it).

### 4.3 Event types and payloads

| `type` | `data` |
|---|---|
| `user_message` | `{ "message": string }` |
| `reasoning_delta` | `{ "delta": string }` |
| `content_delta` | `{ "delta": string }` |
| `tool_call_request` | `{ "id": string, "name": string, "arguments": string }` — `arguments` is a raw JSON string |
| `tool_call_result` | `{ "id": string, "result": string }` |
| `tool_call_error` | `{ "id": string, "error": string }` |
| `tool_call_auto_approved` | `{ "id": string, "name": string, "arguments": string }` |
| `turn_done` | `{ "usage": TokenUsage \| null, "stop_reason": string \| null }` |
| `status_changed` | `{ "status": SessionStatus }` — **defined but not emitted yet**; handle it (update a status field) but don't depend on it |
| `rejected` | `{ "reason": string }` — defined, not emitted yet; render as an error banner when it arrives |
| `resync_required` | `{}` — emitted when the server's broadcast buffer lagged; you may have missed events. Refetch snapshot (6.3). |

Note: the old wire format also had a user `RespondToToolCall` event that the zip's
reducer used to flip a card to running/denied. The new wire has **no such event** —
flip the card optimistically in the client when the user clicks Approve/Deny and the
POST returns 202 (section 6.2).

`TokenUsage` (matches `provider::TokenUsage`, all fields nullable):

```json
{ "prompt_tokens": 28, "completion_tokens": 53, "total_tokens": 81,
  "cached_tokens": 1, "cache_creation_tokens": null }
```

⚠️ The zip uses `input_tokens`/`output_tokens` — those field names are **wrong** for
the new API. Everywhere the zip reads `usage.input_tokens + usage.output_tokens`,
read `usage.total_tokens ?? ((usage.prompt_tokens ?? 0) + (usage.completion_tokens ?? 0))`.

### 4.4 SessionStatus

`ready | generating | resolving_tools | executing_tools | aborted | dead | unknown`.
Until the core publishes live state, `GET /sessions/{id}` always returns `"unknown"`.
Display `unknown` as a neutral dot/label; don't invent state from it.

### 4.5 Snapshot message shape

`messages` are `provider::Message` values. Assumed serialization (externally-tagged,
snake_case — **coordination point** with the server implementer; keep the decoder in
one function so it's a one-place fix if the server's shape differs):

```json
{ "system": "…" }
{ "user": { "message": "…", "tool_call_results": [ { "tool_call_id": "…", "content": "…" } ] } }
{ "assistant": { "reasoning": "…"|null, "content": "…", "tool_calls": [ { "id","name","arguments" } ] } }
```

⚠️ This is NOT the old `SnapshotMessage` (`{type:'user',text}` …) shape from the zip's
`types.ts`, and it is NOT flattened — tool results ride inside the *user* message.

Mapping snapshot → `UiItem[]` (for initial load and resync):

1. `system` → skip.
2. `user` → for each entry in `tool_call_results`, attach `content` as the `result`
   of the matching tool card (by `tool_call_id`) and mark it `done` (or `denied` if
   you tracked a denial locally — after a resync you can't know; `done` is fine).
   Then, if `message` is non-empty, push a user item. (Order matters: results belong
   to the *previous* assistant turn's tool calls.)
3. `assistant` → push an assistant item (`streaming: false`, reasoning ?? '',
   content), then one tool card per `tool_calls` entry with status `running`
   (a later user message's `tool_call_results` will complete them; trailing
   incomplete ones stay `running`/`pending_approval` — use the
   `APPROVAL_REQUIRED_TOOLS` heuristic for trailing calls with no result).

### 4.6 Error envelope

Every non-2xx response body:

```json
{ "error": { "code": "session_not_found", "message": "human readable" } }
```

| code | HTTP |
|---|---|
| `invalid_request` | 400 |
| `session_not_found` | 404 |
| `session_dead` | 410 |
| `not_implemented` | 501 |

Client handling: parse the envelope, surface `message`. On **404/410** for an active
session: mark the session dead in the local registry (grey it out or remove it) and
show a banner. On **410** from `/messages` or `/tool_calls`, the session loop is gone
— disable the composer.

### 4.7 SSE client contract (reconnect)

- No replay. On disconnect: `GET /snapshot`, rebuild transcript, resubscribe to
  `/events`. Use a small backoff (e.g. 1 s, capped ~10 s) to avoid hammering a dead
  server; a 404 on reconnect means the session was deleted — stop.
- Ignore `: keepalive` comment lines (EventSource does this automatically).
- On `resync_required`: same snapshot-rebuild, but **keep the current stream open**.

---

## 5. Delta checklist: zip client code → new spec

Every place the zip's `lib/auger/*` must change:

1. **Routes renamed:** `POST /sessions/{id}/input {content}` → `POST
   /sessions/{id}/messages {message}`. `POST /sessions/{id}/approve {tool_call_id,
   approved, message}` → `POST /sessions/{id}/tool_calls/{call_id} {decision:
   "approve"|"deny", message?}` (call id moves into the path, boolean becomes an
   enum string).
2. **Auth tokens removed:** no `owner_token`/`viewer_token` anywhere. Create response
   is `{session_id, model, created_at}`. Delete every `Bearer` header, `SessionCreds`
   token fields, and the `useAugerSession(sessionId, ownerToken)` second argument.
   The manual fetch-based SSE reader in `api.ts` existed only because EventSource
   can't send headers — with no headers needed, use **native `EventSource`**
   (simpler, free auto-reconnect events; but implement the snapshot-then-resubscribe
   contract yourself rather than trusting its silent retry — call `.close()` on
   error and run your own reconnect loop).
3. **Event format:** old = serde externally-tagged (`{Clanker:{ContentDelta:{delta}}}`);
   new = flat envelope (`{seq,type:"content_delta",data:{delta}}`). Rewrite the
   reducer's event dispatch on `envelope.type` (mapping table in 6.2).
4. **Usage field names:** `input_tokens`/`output_tokens` → `prompt_tokens`/
   `completion_tokens`/`total_tokens` (4.3). Affects `TokenUsage` type, the
   `turn_done` reducer branch, and the status-bar totals in `session-view.tsx`.
5. **Snapshot shape changed** (4.5) — and the zip never actually consumed snapshots;
   the new UI **must** (initial load of an existing session, reconnect, resync).
6. **No `GET /sessions` list.** `page.tsx`'s SWR polling goes away. Replace with a
   localStorage registry (`localStorage['auger.sessions']` = array of
   `{session_id, model, created_at}` saved on create, removed on delete/404). On app
   start, optionally revalidate each entry with `GET /sessions/{id}` and drop 404s.
   The mobile `<select>` and sidebar read from this registry.
7. **No `context_window` in any response.** Zip's `SessionInfo.context_window` and
   the create response's field are gone; use a client-side constant (3.6).
8. **`created_at` is unix seconds** — the zip's `new Date(s.created_at)` needs
   `new Date(s.created_at * 1000)`.
9. **New endpoints to wire:** `GET /sessions/{id}` (revalidation + status),
   `DELETE /sessions/{id}` (3.10), `POST /interrupt` (3.11).
10. **Error envelope parsing** (4.6) instead of `throw new Error(status + text)`.
11. **`RespondToToolCall` user event no longer exists** on the wire → optimistic
    card status update after a 202 (4.3 note).
12. **Base URL:** old default `/v1` mock. Keep `/v1` as the dev-mock default, real
    server via `PUBLIC_AUGER_BASE` (SvelteKit: `$env/static/public`), e.g.
    `PUBLIC_AUGER_BASE=http://127.0.0.1:3000`. (Cross-origin use requires CORS on the
    Rust server — note for the server task, not yours.)

---

## 6. Client modules to write

### 6.1 `src/lib/api.js`

Thin fetch client, JSDoc-typed. `BASE = import.meta.env.PUBLIC_AUGER_BASE ?? '/v1'`.

```
createSession(model?)            POST /sessions                          → {session_id, model, created_at}
getSession(id)                   GET  /sessions/{id}                     → info (or throws ApiError)
deleteSession(id)                DELETE /sessions/{id}                   → void (204)
sendMessage(id, message)         POST /sessions/{id}/messages            → void (202)
respondToToolCall(id, callId, approve, message?)
                                 POST /sessions/{id}/tool_calls/{callId} body {decision, message}
interrupt(id)                    POST /sessions/{id}/interrupt           → void; throws ApiError(code) incl. not_implemented
getSnapshot(id)                  GET  /sessions/{id}/snapshot            → {as_of_seq, messages}
subscribeEvents(id, onEnvelope, onDisconnect)
                                 native EventSource on /sessions/{id}/events;
                                 returns {close()}. Listen with `onmessage` — parse
                                 event.data as the envelope JSON and dispatch on
                                 envelope.type (works regardless of the `event:` name);
                                 skip unparseable frames.
```

All non-2xx → throw `ApiError { code, message, status }` from the error envelope
(fall back to `{code:'invalid_request', message: status text}` if the body isn't the
envelope).

### 6.2 `src/lib/session.svelte.js` — the state machine

Port of `use-auger-session.ts`'s reducer as a Svelte 5 class using `$state`. Public
shape:

```js
class AugerSession {
  items = $state([])            // UiItem[]  (same shape as zip's types.ts UiItem)
  busy = $state(false)
  connected = $state(false)
  status = $state('unknown')    // SessionStatus, fed by status_changed / GET info
  error = $state(null)
  totalUsage = $state({ prompt_tokens: 0, completion_tokens: 0 })
  contextTokens = $state(0)

  constructor(sessionId)
  async start()                 // snapshot → build items → subscribe
  stop()                        // close stream
  async send(text)              // sendMessage; on 410 → mark dead, surface error
  async respond(callId, approve, message?)  // POST then optimistic card flip (see below)
  async interrupt()             // 501 → this.error = 'interrupt not supported…' (non-fatal)
}
```

Envelope → reducer mapping (logic identical to the zip reducer unless noted):

| envelope.type | zip reducer branch | notes |
|---|---|---|
| `user_message` | `User.SendMessage` | payload is `data.message` (not `.msg`); push user item, `busy = true`. Skip if it matches an optimistically-added local user item (dedupe by text of the most recent pending local echo, or simply don't echo locally and rely on this event — pick one strategy and be consistent; relying on the event alone is simpler and is what the zip does). |
| `reasoning_delta` / `content_delta` | `Clanker.ReasoningDelta/ContentDelta` | unchanged: append to last streaming assistant item or open one. |
| `tool_call_request` | `Clanker.ToolCallRequest` | unchanged: close streaming bubble, add card (`pending_approval` if name ∈ APPROVAL_REQUIRED_TOOLS else `running`), dedupe by call id. |
| `tool_call_auto_approved` | `ToolCall.AutoApproved` | unchanged: upgrade/create card as running+auto. |
| `tool_call_result` | `ToolCall.Result` | unchanged → `done`. |
| `tool_call_error` | `ToolCall.Error` | unchanged → `error` (keep `denied` if already denied). |
| `turn_done` | `Clanker.Done` | finalize bubble; `busy = false`; accumulate usage with **new field names** (4.3); `contextTokens = usage.total_tokens ?? prompt+completion`. |
| `status_changed` | — (new) | `this.status = data.status`; if `aborted`, also `busy = false` and close the streaming bubble. |
| `rejected` | — (new) | `this.error = data.reason`. |
| `resync_required` | — (new) | trigger resync (6.3). |
| unknown type | — | ignore silently (forward-compat). |

Approve/Deny (replaces the old `RespondToToolCall` event handling): after
`respondToToolCall()` resolves without throwing, set the card to `running` (approve)
or `denied` (deny). On `ApiError` 410 → session dead handling; other errors →
`this.error`.

### 6.3 Connection lifecycle

- `start()`: `getSnapshot()` → build items via the 4.5 mapping → `subscribeEvents()`
  → `connected = true`.
- Stream error/close (and not intentionally stopped): `connected = false`, backoff,
  then re-run the snapshot+subscribe sequence. 404 during reconnect → mark session
  removed, stop.
- `resync_required`: refetch snapshot and **replace** `items` (stream stays open).
  Guard against overlapping resyncs (a simple in-flight flag).

### 6.4 `src/lib/session-list.svelte.js`

localStorage-backed registry (5.6): `sessions` ($state array), `add(info)`,
`remove(id)`, `revalidate()` (GET each; drop 404s; update status). Persist on every
mutation; hydrate in the browser only (guard `typeof localStorage`).

---

## 7. Component porting notes (React → Svelte 5)

General: `useState` → `$state`; derived values → `$derived`; `useEffect` →
`$effect`; `props` → `$props()`; `onClick` → `onclick`; `className` → `class`;
`cn(a, cond && b)` → template class strings or `class:`. Keep the Tailwind class
strings **verbatim** from the tsx files — they encode the whole design.

1. **`+page.svelte`** (from `app/page.tsx`): owns `sessionList`, `activeId`,
   `creating`. `handleCreate(model)` → `createSession` → `list.add` → set active.
   Welcome screen inline or as a small component; keep the ASCII banner exactly
   (watch backslash escaping in a Svelte `{@html}`-free `<pre>` — use `{String.raw}`
   equivalent: just paste literal text inside `<pre>`, escaping backticks not needed
   in Svelte, but `\` characters are fine in plain markup).
2. **`SessionSidebar.svelte`**: model `<select>` — replace the zip's stale hardcoded
   `MODELS` list with a single default (server default applies when omitted) plus a
   free-text override, or keep a hardcoded list if that's simplest; sessions come
   from the registry; add the delete button (3.10); `created_at * 1000` for the time
   (5.8).
3. **`SessionView.svelte`**: instantiate `new AugerSession(id)` on mount (or
   `$effect` keyed by session id — recreate + `stop()` old on change; the zip used
   `key={session_id}` remount semantics, easiest parallel is `{#key sessionId}` in
   the parent). Pinned-scroll via a `bind:this` container + `onscroll` handler +
   `$effect` on `items`. Status bar totals with new usage fields.
4. **`Messages.svelte`**: straightforward; reasoning collapse is local `$state`.
5. **`ToolCallCard.svelte`**: port as-is (icons map, `parseArgs`, `argSummary`,
   badges, auto-open when pending). For `edit` calls with `old_string`/`new_string`
   in args, render the existing **`DiffViewer.svelte`**.
6. **`DiffViewer.svelte` (existing)**: keep. It takes whatever props it currently
   takes — check its interface (`src/lib/DiffViewer.svelte`) and adapt the call site,
   not the component. If its visual style clashes badly with the new theme, fall back
   to porting the zip's simple table diff instead (10 minutes, zero deps).
7. **Markdown**: reuse `src/lib/markdown.js` (markdown-it render to HTML string,
   `{@html …}` in a wrapper div). Add a scoped style block (or Tailwind `prose`-like
   utility classes on the wrapper with `:global`) that reproduces the element styling
   from `markdown.tsx`: h1→h3-size, tight lists, primary-colored inline code on
   `--muted` chips, bordered code blocks on `--sidebar` background, primary
   underlined links opening in new tabs (markdown-it plugin or `target` rewrite).
8. **`Composer.svelte`**: port as-is; add the Stop/interrupt button shown while
   `busy` (3.11).

---

## 8. New mock server (SvelteKit `src/routes/v1/**`)

Purpose: develop/demo the UI with `vite dev` before the Rust server exists. Implement
the **spec exactly** (routes, envelopes, error envelope, status codes) so switching
`PUBLIC_AUGER_BASE` to the real server is a no-op. The old `src/lib/mock-store.js`
and the zip's `mock-server.ts` are shape-incompatible — write fresh, but steal their
scripted-conversation content if convenient.

Requirements:

- In-memory `Map<session_id, MockSession>` in `src/lib/mock/engine.js` (module-level;
  fine for dev).
- `POST /v1/sessions` → 201 with spec body. `GET` info route returns `status:
  "unknown"`, `pending_tool_calls: []`. `DELETE` → 204/404 and closes any open SSE
  streams for that session.
- `POST …/messages` → 202, then asynchronously emits a scripted turn over SSE:
  `user_message` → several `reasoning_delta` → `content_delta` chunks → a
  `tool_call_request` for `shell` (exercises approval UI) and one auto tool (e.g.
  `read` + `tool_call_auto_approved` + `tool_call_result`) → after approval arrives
  via `POST …/tool_calls/{id}` emit `tool_call_result` (or `tool_call_error` with
  "denied by user" on deny) → closing `content_delta`s → `turn_done` with a
  plausible `TokenUsage` (new field names!). Include one `edit` tool call with
  `old_string`/`new_string` args somewhere so the diff renders.
- `…/interrupt` → **501** `{error:{code:"not_implemented",…}}` (mirrors the real
  server's stub phase).
- `…/events` → `ReadableStream` SSE: send `event:` + `data:` lines per envelope,
  `: keepalive\n\n` every 15 s, register/unregister the subscriber on the session.
- `…/snapshot` → build `provider::Message`-shaped messages (4.5) from the mock's
  accumulated history so reconnect/resync paths are testable.
- Error envelope + correct codes on all failure paths (404 unknown id, 410 for a
  "dead" session — e.g. keep deleted ids in a tombstone set and return 410 from
  message/tool_call posts… actually spec says 404 after DELETE removal; use 404 for
  deleted, and fake a 410 only if you add a "kill" debug hook. Don't overbuild —
  404 everywhere for unknown is acceptable for the mock).

---

## 9. package.json changes

Add: `tailwindcss` + `@tailwindcss/vite` (dev), `lucide-svelte`.
Optionally: `@fontsource-variable/geist`, `@fontsource-variable/geist-mono`.
Remove if now unused after the port: `diff`, `@types/diff` (check whether
`DiffViewer.svelte` uses them before removing), `@vscode/markdown-it-katex`/`katex`
(keep — markdown.js uses them).
Do **not** add: react anything, swr, next, shadcn, tw-animate-css.

---

## 10. Acceptance checklist

- [ ] `pnpm dev` serves the new UI against the built-in `/v1` mock with zero console errors.
- [ ] Create session → appears in sidebar + localStorage; survives page reload (revalidated via `GET /sessions/{id}`).
- [ ] Send message → user line, streaming reasoning (collapsible) + content with blinking caret, pinned scroll behavior (scroll up = unpinned).
- [ ] `shell` tool call renders pending-approval card, auto-opened; Approve → running → done with output; Deny → denied badge (+ error text when `tool_call_error` arrives).
- [ ] `edit` tool call renders a diff; auto tools show `auto` badge.
- [ ] `turn_done` clears busy and updates token totals + context bar using `prompt_tokens`/`completion_tokens`/`total_tokens`.
- [ ] Kill/restart dev server mid-session → UI reconnects: snapshot rebuild, no duplicated items.
- [ ] `resync_required` (add a mock debug trigger) → transcript rebuilt from snapshot, stream stays connected.
- [ ] Delete session → 204, removed from sidebar and storage.
- [ ] Interrupt button while busy → 501 handled with a friendly non-fatal notice.
- [ ] Non-2xx responses render the error envelope's `message`; 410 disables the composer.
- [ ] `pnpm check` (svelte-check) passes; no React/Next/SWR deps in package.json.

## 11. Open coordination points (flag, don't solve)

1. **Snapshot serialization of `provider::Message`** — the Rust `provider::Message`
   currently has **no `Serialize` derive**, so the server team must define the wire
   shape; 4.5 documents the assumed shape. Keep the frontend decoder in one function.
2. **CORS** on the Rust server if the webui is served from a different origin.
3. **Context window** is not exposed by the API; UI uses a constant until a
   models/info endpoint exists (explicitly out of scope in the spec).
4. `status_changed` / `rejected` / `seq` / `as_of_seq` are dormant; the UI handles
   them but nothing emits them yet.
