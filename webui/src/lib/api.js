// Thin client for the auger agent-server HTTP API.
//
// Endpoints (see agent-server/src/main.rs):
//   POST /v1/sessions                 -> { session_id, owner_token, viewer_token }
//   POST /v1/sessions/{id}/input      (Bearer owner) { content } -> 202
//   POST /v1/sessions/{id}/approve    (Bearer owner) { tool_call_id, approved } -> 200
//   GET  /v1/sessions/{id}/events     (Bearer any)   -> SSE of AgentEvent
//
// EventSource can't set an Authorization header, so the SSE stream is consumed
// with fetch + a ReadableStream reader and parsed manually.

const BASE = '/v1';

/**
 * @typedef {Object} SessionCreds
 * @property {string} session_id
 * @property {string} owner_token
 * @property {string} viewer_token
 */

/**
 * @param {string|undefined} model
 * @returns {Promise<SessionCreds>}
 */
export async function createSession(model) {
	const res = await fetch(`${BASE}/sessions`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify({ model: model || null })
	});
	if (!res.ok) throw new Error(`createSession failed: ${res.status}`);
	return res.json();
}

/**
 * @param {string} id
 * @param {string} ownerToken
 * @param {string} content
 */
export async function sendInput(id, ownerToken, content) {
	const res = await fetch(`${BASE}/sessions/${id}/input`, {
		method: 'POST',
		headers: {
			'content-type': 'application/json',
			authorization: `Bearer ${ownerToken}`
		},
		body: JSON.stringify({ content })
	});
	if (!res.ok) throw new Error(`sendInput failed: ${res.status} ${await res.text()}`);
}

/**
 * @param {string} id
 * @param {string} ownerToken
 * @param {string} toolCallId
 * @param {boolean} approved
 */
export async function approveTool(id, ownerToken, toolCallId, approved) {
	const res = await fetch(`${BASE}/sessions/${id}/approve`, {
		method: 'POST',
		headers: {
			'content-type': 'application/json',
			authorization: `Bearer ${ownerToken}`
		},
		body: JSON.stringify({ tool_call_id: toolCallId, approved })
	});
	if (!res.ok) throw new Error(`approveTool failed: ${res.status} ${await res.text()}`);
}

/**
 * Subscribe to a session's SSE event stream.
 * Returns an AbortController; call `.abort()` to disconnect.
 *
 * @param {string} id
 * @param {string} token  owner or viewer token
 * @param {(event: any) => void} onEvent  receives each parsed AgentEvent
 * @param {(err: Error) => void} [onError]
 * @returns {AbortController}
 */
export function subscribeEvents(id, token, onEvent, onError) {
	const controller = new AbortController();

	(async () => {
		try {
			const res = await fetch(`${BASE}/sessions/${id}/events`, {
				headers: { authorization: `Bearer ${token}` },
				signal: controller.signal
			});
			if (!res.ok || !res.body) {
				throw new Error(`events stream failed: ${res.status}`);
			}

			const reader = res.body.getReader();
			const decoder = new TextDecoder();
			let buffer = '';

			while (true) {
				const { done, value } = await reader.read();
				if (done) break;
				buffer += decoder.decode(value, { stream: true });

				// SSE frames are separated by a blank line.
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
		} catch (err) {
			if (controller.signal.aborted) return;
			onError?.(err instanceof Error ? err : new Error(String(err)));
		}
	})();

	return controller;
}

/**
 * Extract the concatenated `data:` payload from one SSE frame.
 * @param {string} frame
 * @returns {string|null}
 */
function parseFrame(frame) {
	const lines = frame.split('\n');
	const data = [];
	for (const line of lines) {
		if (line.startsWith('data:')) data.push(line.slice(5).trimStart());
	}
	return data.length ? data.join('\n') : null;
}
