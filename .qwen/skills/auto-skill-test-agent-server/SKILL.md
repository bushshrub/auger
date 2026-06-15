---
name: test-agent-server
description: End-to-end test the agent server via curl, including session creation, event streaming, tool call approval, and cleanup.
source: auto-skill
extracted_at: '2026-06-15T01:20:23.845Z'
---

## Procedure

### 1. Start the server

```bash
cargo run -p agent-server 2>&1
```

Runs on `127.0.0.1:3000` by default (configurable via `LISTEN_ADDR`).

### 2. Create a session

```bash
curl -s http://127.0.0.1:3000/v1/sessions \
  -X POST -H 'Content-Type: application/json' -d '{}'
```

Returns `session_id`, `owner_token`, and `viewer_token`. Save these for subsequent calls.

### 3. Start the SSE event stream (background)

```bash
curl -s -N -H "Authorization: Bearer $VIEWER_TOKEN" \
  http://127.0.0.1:3000/v1/sessions/$SESSION_ID/events
```

Run in background so you can watch events while interacting with the session.

### 4. Send user input

```bash
curl -s -X POST \
  -H "Authorization: Bearer $OWNER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"content":"your prompt here"}' \
  http://127.0.0.1:3000/v1/sessions/$SESSION_ID/input
```

Returns HTTP 202 Accepted.

### 5. Watch for tool_call events and approve

Monitor the event stream output for `{"type":"tool_call"}` events. When one arrives, approve (or deny):

```bash
curl -s -X POST \
  -H "Authorization: Bearer $OWNER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"tool_call_id":"<id from event>","approved":true}' \
  http://127.0.0.1:3000/v1/sessions/$SESSION_ID/approve
```

The agent pauses at `AwaitingApproval` status until you respond. Repeat for each tool call.

### 6. Watch the final response

After all tool calls are processed, the agent streams `content` events (character-by-character) followed by `turn_complete`.

### 7. Cleanup

```bash
kill <server_pid>
```

## Key events to watch for

| Event | Meaning |
|---|---|
| `tool_call` | Agent wants to use a tool — requires approval |
| `tool_result` | Tool executed — result returned to agent |
| `content` | Agent streaming its answer |
| `turn_complete` | Turn finished, session is idle |
| `error` | Something went wrong |
