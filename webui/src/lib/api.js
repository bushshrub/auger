// Thin client for the auger agent-server HTTP API (agent-server/openapi.yaml).
//
// Endpoints:
//   GET    /sessions               -> { sessions: [{ session_id, model, created_at,
//                                        context_window, tokens: { read, write },
//                                        archived: bool }] }
//   POST   /sessions               -> { session_id, context_window, tokens }
//   DELETE /sessions/{id}          (Bearer write) -> 204 (archives the session)
//   POST   /sessions/{id}/input    (Bearer write) { input } -> { status: "ok" }
//   POST   /sessions/{id}/tool     (Bearer write) { tool_call_id, approved, message? }
//   POST   /sessions/{id}/interrupt (Bearer write) -> { status: "ok" } (fire-and-forget)
//   GET    /sessions/{id}/events   (Bearer read)  -> SSE, one SessionEvent JSON per frame
//   GET    /sessions/{id}/snapshot (Bearer read)  -> TraceRecord[]
//
// EventSource can't set an Authorization header, so the SSE stream is consumed
// with fetch + a ReadableStream reader and parsed manually.
//
// BASE defaults to /v1, which the vite dev server proxies to AGENT_SERVER_URL
// (see vite.config.js). Set VITE_AUGER_BASE to talk to a server directly.

const BASE = import.meta.env.VITE_AUGER_BASE ?? '/v1';

/**
 * @typedef {{ read: string, write: string }} SessionTokens
 * @typedef {{ session_id: string, model: string, created_at: string,
 *             context_window: number, tokens: SessionTokens, archived: boolean }} SessionInfo
 * @typedef {{ session_id: string, model: string, context_window: number, tokens: SessionTokens }} SessionCreds
 * @typedef {{ id: string, name: string, arguments: string }} ToolCall
 * @typedef {{ prompt_tokens: number | null, completion_tokens: number | null,
 *             total_tokens: number | null, cached_tokens: number | null,
 *             cache_creation_tokens: number | null }} TokenUsage
 *
 * A single piece of tool content. Externally tagged: `{ text: { text } }`.
 * @typedef {{ text: { text: string } }} ToolData
 *
 * The outcome of a tool call. Externally tagged, mirroring the Rust enum:
 * `success`/`error` carry content, `denied` carries a reason, `interrupted`
 * is the bare string.
 * @typedef {(
 *   | { success: { content: ToolData[] } }
 *   | { error: { error: ToolData[] } }
 *   | { denied: { reason: string | null } }
 *   | 'interrupted'
 * )} ToolOutcome
 * @typedef {{ tool_call_id: string, outcome: ToolOutcome }} ToolCallResult
 *
 * One SSE frame; discriminated by `type`. `tool_call_result` carries the full
 * structured result (with its outcome); there is no separate error frame — a
 * failed/denied/interrupted call arrives as a `tool_call_result` whose outcome
 * is `error`/`denied`/`interrupted`.
 * @typedef {(
 *   | { type: 'text_delta', text: string }
 *   | { type: 'reasoning_delta', text: string }
 *   | { type: 'tool_call', id: string, name: string, arguments: string }
 *   | { type: 'tool_call_complete', id: string, name: string, arguments: string }
 *   | { type: 'tool_consent_required', tool_calls: ToolCall[] }
 *   | { type: 'tool_call_result', id: string, result: ToolCallResult }
 *   | { type: 'done', usage: TokenUsage | null, stop_reason: string | null }
 *   | { type: 'interrupted' }
 *   | { type: 'stream_error', error: string }
 *   | { type: 'closed' }
 * )} SessionEvent
 *
 * The snapshot is the same record stream persisted to trace.jsonl: a flat,
 * ordered list of TraceRecords, each discriminated by `kind`. It arrives as
 * newline-delimited JSON (one record per line), parsed by getSnapshot into an
 * array. The session record is always first, followed by turns (which carry the
 * conversation) and events (`tool_call_requested`, `tool_authorization`,
 * `tool_call_result`) that occurred during the owning assistant turn. Nested
 * enums are externally tagged, matching the Rust serialization. `arguments` on
 * a tool call is a JSON-encoded string.
 *
 * @typedef {{ type: 'text', text: string } | { type: 'tool_result', tool_call_id: string,
 *             content: ToolData[] }} InputContent
 * @typedef {{ reasoning: string | null, content: string, tool_calls: ToolCall[] }} AssistantResponse
 * @typedef {(
 *   | { completed: { response: AssistantResponse } }
 *   | { interrupted: { partial_response: AssistantResponse | null } }
 *   | 'failed'
 * )} AssistantTurnOutcome
 * @typedef {(
 *   | { kind: 'session', version: number, session_id: string, created_at: string,
 *       cwd: string, model_info: { provider: string, id: string } }
 *   | { kind: 'turn', turn_id: string, timestamp: string, parent_id: string | null,
 *       turn: { input_message: { content: InputContent[] } }
 *           | { assistant_message: { outcome: AssistantTurnOutcome } } }
 *   | { kind: 'event', turn_id: string, parent_id: string | null, timestamp: string,
 *       event_id: string, event:
 *         | { tool_call_requested: { tool_call_id: string, tool_name: string, arguments: string } }
 *         | { tool_authorization: { tool_call_id: string, decision: 'approved' | 'denied',
 *             source: 'user' | 'policy', reason: string | null } }
 *         | { tool_call_result: ToolCallResult } }
 * )} TraceRecord
 */

export class ApiError extends Error {
	/**
	 * @param {number} status
	 * @param {string} message
	 */
	constructor(status, message) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

/**
 * @param {Response} res
 * @returns {Promise<ApiError>}
 */
async function toError(res) {
	const text = await res.text().catch(() => '');
	return new ApiError(res.status, text || res.statusText);
}

/** @returns {Promise<{ sessions: SessionInfo[] }>} */
export async function listSessions() {
	const res = await fetch(`${BASE}/sessions`);
	if (!res.ok) throw await toError(res);
	return res.json();
}

/**
 * @param {string} [model]
 * @returns {Promise<SessionCreds>}
 */
export async function createSession(model) {
	const res = await fetch(`${BASE}/sessions`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify({ model: model || null })
	});
	if (!res.ok) throw await toError(res);
	return res.json();
}

/**
 * Archive a session. The server returns 204; the session remains in GET /sessions
 * with archived: true.
 * @param {string} id
 * @param {string} writeToken
 * @returns {Promise<void>}
 */
export async function archiveSession(id, writeToken) {
	const res = await fetch(`${BASE}/sessions/${id}`, {
		method: 'DELETE',
		headers: { authorization: `Bearer ${writeToken}` }
	});
	if (!res.ok) throw await toError(res);
}

/**
 * @param {string} id
 * @param {string} writeToken
 * @param {string} input
 * @returns {Promise<void>}
 */
export async function sendInput(id, writeToken, input) {
	const res = await fetch(`${BASE}/sessions/${id}/input`, {
		method: 'POST',
		headers: {
			'content-type': 'application/json',
			authorization: `Bearer ${writeToken}`
		},
		body: JSON.stringify({ input })
	});
	if (!res.ok) throw await toError(res);
}

/**
 * @param {string} id
 * @param {string} writeToken
 * @param {string} toolCallId
 * @param {boolean} approved
 * @param {string} [message]
 * @returns {Promise<void>}
 */
export async function respondToToolCall(id, writeToken, toolCallId, approved, message) {
	/** @type {Record<string, unknown>} */
	const body = { tool_call_id: toolCallId, approved };
	if (message) body.message = message;
	const res = await fetch(`${BASE}/sessions/${id}/tool`, {
		method: 'POST',
		headers: {
			'content-type': 'application/json',
			authorization: `Bearer ${writeToken}`
		},
		body: JSON.stringify(body)
	});
	if (!res.ok) throw await toError(res);
}

/**
 * Interrupt in-flight generation or tool execution. Fire-and-forget: the
 * outcome arrives on the event stream (`interrupted` / `tool_call_result`).
 * @param {string} id
 * @param {string} writeToken
 * @returns {Promise<void>}
 */
export async function interruptSession(id, writeToken) {
	const res = await fetch(`${BASE}/sessions/${id}/interrupt`, {
		method: 'POST',
		headers: { authorization: `Bearer ${writeToken}` }
	});
	if (!res.ok) throw await toError(res);
}

/**
 * Fetch the session snapshot. The response is newline-delimited JSON
 * (application/x-ndjson), one TraceRecord per line — not a JSON array — so it
 * is parsed line by line here.
 * @param {string} id
 * @param {string} token
 * @returns {Promise<TraceRecord[]>}
 */
export async function getSnapshot(id, token) {
	const res = await fetch(`${BASE}/sessions/${id}/snapshot`, {
		headers: { authorization: `Bearer ${token}` }
	});
	if (!res.ok) throw await toError(res);
	const body = await res.text();
	return body
		.split('\n')
		.filter((line) => line.trim().length > 0)
		.map((line) => JSON.parse(line));
}

/**
 * Subscribe to the session event stream. Returns an AbortController; abort it
 * to close the stream. `onClose` fires when the stream ends or errors (but not
 * on deliberate abort) so the caller can run its reconnect logic.
 *
 * @param {string} id
 * @param {string} token
 * @param {(event: SessionEvent) => void} onEvent
 * @param {(err: Error) => void} [onClose]
 * @returns {AbortController}
 */
export function subscribeEvents(id, token, onEvent, onClose) {
	const controller = new AbortController();

	(async () => {
		try {
			const res = await fetch(`${BASE}/sessions/${id}/events`, {
				headers: { authorization: `Bearer ${token}` },
				signal: controller.signal
			});
			if (!res.ok || !res.body) throw await toError(res);

			const reader = res.body.getReader();
			const decoder = new TextDecoder();
			let buffer = '';

			for (;;) {
				const { done, value } = await reader.read();
				if (done) break;
				buffer += decoder.decode(value, { stream: true });

				let sep;
				while ((sep = buffer.indexOf('\n\n')) !== -1) {
					const frame = buffer.slice(0, sep);
					buffer = buffer.slice(sep + 2);
					const data = parseFrame(frame);
					if (data === null) continue;
					try {
						onEvent(JSON.parse(data));
					} catch {
						// non-JSON keepalive or comment; ignore
					}
				}
			}
			if (!controller.signal.aborted) {
				onClose?.(new Error('event stream ended'));
			}
		} catch (err) {
			if (controller.signal.aborted) return;
			onClose?.(err instanceof Error ? err : new Error(String(err)));
		}
	})();

	return controller;
}

/**
 * @param {string} frame
 * @returns {string | null}
 */
function parseFrame(frame) {
	const lines = frame.split('\n');
	const data = [];
	for (const line of lines) {
		if (line.startsWith('data:')) data.push(line.slice(5).trimStart());
	}
	return data.length ? data.join('\n') : null;
}
