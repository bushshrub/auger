# llama.cpp OpenAI-Compatible API Reference

Server: `http://server-slop:8081` | Model: `gemma4-12b` | No API key required.

## Commands

### List Models

```bash
curl -s http://server-slop:8081/v1/models | jq
```

### Chat Completion (Non-Streaming)

```bash
curl -s http://server-slop:8081/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemma4-12b",
    "messages": [
      {"role": "user", "content": "Hello"}
    ],
    "temperature": 0.7
  }' | jq
```

### Chat Completion (Streaming)

```bash
curl -s http://server-slop:8081/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemma4-12b",
    "messages": [
      {"role": "user", "content": "Hello"}
    ],
    "stream": true,
    "temperature": 0.7
  }'
```

## Request Schema

### `/v1/chat/completions` — POST

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `model` | string | yes | — | Model name (must match loaded model) |
| `messages` | array | yes | — | `[{role, content}, ...]` roles: `system`, `user`, `assistant` |
| `stream` | bool | no | `false` | If true, returns SSE stream |
| `temperature` | number | no | `0.8` | Sampling temperature |

## Response Schemas

### Non-Streaming Response — `ChatCompletion`

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1781475209,
  "model": "gemma4-12b",
  "system_fingerprint": "b9602-...",
  "choices": [{
    "index": 0,
    "message": {
      "role": "assistant",
      "content": "Final answer text.",
      "reasoning_content": "Internal reasoning/thinking steps."
    },
    "finish_reason": "stop"
  }],
  "usage": {
    "prompt_tokens": 28,
    "completion_tokens": 53,
    "total_tokens": 81,
    "prompt_tokens_details": { "cached_tokens": 1 }
  },
  "timings": {
    "prompt_n": 27,
    "prompt_ms": 89.064,
    "prompt_per_token_ms": 3.29,
    "prompt_per_second": 303.15,
    "predicted_n": 53,
    "predicted_ms": 427.13,
    "predicted_per_token_ms": 8.05,
    "predicted_per_second": 124.08,
    "draft_n": 46,
    "draft_n_accepted": 31,
    "cache_n": 1
  }
}
```

### Streaming Response — SSE Chunks

Each line is `data: <json>`, final line is `data: [DONE]`.

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion.chunk",
  "created": 1781475119,
  "model": "gemma4-12b",
  "system_fingerprint": "b9602-...",
  "choices": [{
    "index": 0,
    "delta": {
      "role": "assistant",
      "content": "...",
      "reasoning_content": "..."
    },
    "finish_reason": null
  }]
}
```

**Delta fields:**
- First chunk: `delta.role: "assistant"`, `content: null`
- Middle chunks: `delta.content` OR `delta.reasoning_content` (partial text)
- Final chunk: `finish_reason: "stop"`, `delta: {}`, includes `timings` block

## Key Notes

- **Reasoning content:** `gemma4-12b` emits `reasoning_content` tokens (chain-of-thought) before `content`. Both appear in the same `message`/`delta` object — check for both fields.
- **Speculative decoding:** Uses draft tokens (`draft_n` / `draft_n_accepted` in timings).
- **Timing fields:** llama.cpp adds a `timings` object with prompt/generation benchmarks and cache info.
- **No auth:** Server runs without API keys.
- **Model field:** Must match the loaded model name; use `/v1/models` to discover it.
