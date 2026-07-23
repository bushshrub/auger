# Auger frontend API and trace contracts

## Trace record stream

The persistent `trace.jsonl` file and the session snapshot endpoint use the
same format: newline-delimited JSON with one complete trace record per line.

Every record has a `kind` discriminator:

```text
"session" | "turn" | "event"
```

All enum variant names are snake_case.

### Session record

The session record is always first and is written once:

```json
{
  "kind": "session",
  "version": 1,
  "session_id": "<uuid>",
  "created_at": "<RFC3339 timestamp>",
  "cwd": "<path>",
  "model_info": {
    "provider": "<provider>",
    "id": "<model id>"
  }
}
```

### Input turn

```json
{
  "kind": "turn",
  "turn_id": "<uuid>",
  "timestamp": "<RFC3339 timestamp>",
  "parent_id": null,
  "turn": {
    "input_message": {
      "content": [
        {
          "type": "text",
          "text": "..."
        },
        {
          "type": "tool_result",
          "tool_call_id": "...",
          "content": [
            {
              "text": {
                "text": "..."
              }
            }
          ]
        }
      ]
    }
  }
}
```

`parent_id` is `null` for the first turn. Later turns contain the UUID of the
immediately preceding turn.

### Completed assistant turn

```json
{
  "kind": "turn",
  "turn_id": "<uuid>",
  "timestamp": "<RFC3339 timestamp>",
  "parent_id": "<previous turn uuid>",
  "turn": {
    "assistant_message": {
      "outcome": {
        "completed": {
          "response": {
            "reasoning": null,
            "content": "...",
            "tool_calls": [
              {
                "id": "...",
                "name": "...",
                "arguments": "<raw JSON string>"
              }
            ]
          }
        }
      }
    }
  }
}
```

An interrupted assistant outcome contains an optional partial response:

```json
{
  "interrupted": {
    "partial_response": {
      "reasoning": null,
      "content": "...",
      "tool_calls": []
    }
  }
}
```

`partial_response` may be `null`. A failed assistant outcome is the string:

```json
"failed"
```

### Event record

Events are separate records and refer to their owning assistant turn:

```json
{
  "kind": "event",
  "turn_id": "<owning turn uuid>",
  "parent_id": null,
  "timestamp": "<RFC3339 timestamp>",
  "event_id": "<uuid>",
  "event": {
    "tool_call_requested": {
      "tool_call_id": "...",
      "tool_name": "...",
      "arguments": "<raw JSON string>"
    }
  }
}
```

Tool authorization:

```json
{
  "tool_authorization": {
    "tool_call_id": "...",
    "decision": "approved",
    "source": "user",
    "reason": null
  }
}
```

`decision` is `"approved"` or `"denied"`. `source` is `"user"` or
`"policy"`.

Tool result:

```json
{
  "tool_call_result": {
    "tool_call_id": "...",
    "outcome": {
      "success": {
        "content": [
          {
            "text": {
              "text": "..."
            }
          }
        ]
      }
    }
  }
}
```

Other tool outcomes:

```json
{ "error": { "error": [{ "text": { "text": "..." } }] } }
```

```json
{ "denied": { "reason": null } }
```

```json
"interrupted"
```

### Ordering and identity

- The session record must be first.
- Turn IDs and event IDs must be unique.
- Turns form a linear chain through `parent_id`.
- Events can be appended incrementally after their owning turn.
- An event parent must refer to an event that appeared earlier.

## `GET /sessions/{id}/snapshot`

The snapshot response is the complete trace record stream described above.
Its content type is:

```text
application/x-ndjson
```

The response is not a JSON array and is not wrapped in a `messages` object.
Parse it one non-empty line at a time:

```text
records = body.lines()
    .filter(line is not empty)
    .map(JSON.parse)
```

The first parsed record is the session record. Remaining records are turns and
events in trace order.

## `GET /sessions/{id}/events` SSE

SSE is the live runtime event transport. It does not send raw trace records.
Each SSE `data` field contains one JSON object.

Text and reasoning deltas:

```json
{ "type": "text_delta", "text": "..." }
```

```json
{ "type": "reasoning_delta", "text": "..." }
```

Partial and completed tool calls:

```json
{
  "type": "tool_call",
  "id": "...",
  "name": "...",
  "arguments": "<possibly incomplete JSON string>"
}
```

```json
{
  "type": "tool_call_complete",
  "id": "...",
  "name": "...",
  "arguments": "<complete JSON string>"
}
```

Stream completion:

```json
{
  "type": "done",
  "usage": {
    "prompt_tokens": null,
    "completion_tokens": null,
    "total_tokens": null,
    "cached_tokens": null,
    "cache_creation_tokens": null
  },
  "stop_reason": null
}
```

`usage` may be `null`. Individual token counts and `stop_reason` may also be
`null`.

Tool consent:

```json
{
  "type": "tool_consent_required",
  "tool_calls": [
    {
      "id": "...",
      "name": "...",
      "arguments": "<raw JSON string>"
    }
  ]
}
```

Tool result:

```json
{
  "type": "tool_call_result",
  "id": "...",
  "result": {
    "tool_call_id": "...",
    "outcome": {
      "success": {
        "content": [
          {
            "text": {
              "text": "..."
            }
          }
        ]
      }
    }
  }
}
```

Lifecycle events:

```json
{ "type": "interrupted" }
```

```json
{ "type": "stream_error", "error": "..." }
```

```json
{ "type": "closed" }
```

Use the snapshot trace stream to hydrate session state and the SSE stream to
apply live updates.
