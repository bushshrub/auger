import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;
const enc = new TextEncoder();

/**
 * Map a server SessionEvent (externally-tagged Rust enum) to UI event shape(s).
 *
 * Server shape (serde externally-tagged enums):
 *   { "Clanker": { "ContentDelta": { "delta": "..." } } }
 *   { "Clanker": { "ReasoningDelta": { "delta": "..." } } }
 *   { "Clanker": { "ToolCallRequest": { "id", "name", "arguments" } } }
 *   { "Clanker": { "Done": { "usage": { "prompt_tokens", "completion_tokens" }, "stop_reason" } } }
 *   { "ToolCall": { "Result": { "id", "result" } } }
 *   { "ToolCall": { "Error": { "id", "error" } } }
 *   { "ToolCall": { "AutoApproved": { "id", "name", "arguments" } } }
 *   { "User": ... }  (dropped — tracked locally by the UI)
 *
 * UI: { type: string, data: any }
 *
 * Returns an array (may be empty or multiple events, e.g. Done emits metrics + turn_complete).
 *
 * @param {any} ev
 * @returns {any[]}
 */
function transformEvent(ev) {
	if (ev.Clanker) {
		const c = ev.Clanker;
		if (c.ContentDelta) return [{ type: 'content', data: { text: c.ContentDelta.delta } }];
		if (c.ReasoningDelta) return [{ type: 'reasoning', data: { text: c.ReasoningDelta.delta } }];
		if (c.ToolCallRequest) {
			let args = c.ToolCallRequest.arguments;
			try { args = JSON.parse(args); } catch { /* keep as string */ }
			return [{ type: 'tool_call', data: { id: c.ToolCallRequest.id, name: c.ToolCallRequest.name, arguments: args } }];
		}
		if (c.Done) {
			const out = [];
			if (c.Done.usage) {
				out.push({ type: 'metrics', data: {
					prompt_tokens: c.Done.usage.prompt_tokens,
					completion_tokens: c.Done.usage.completion_tokens,
					total_tokens: c.Done.usage.total_tokens,
				}});
			}
			out.push({ type: 'turn_complete', data: {} });
			return out;
		}
	}
	if (ev.ToolCall) {
		const t = ev.ToolCall;
		if (t.Result) {
			return [{ type: 'tool_result', data: { id: t.Result.id, content: t.Result.result } }];
		}
		if (t.Error) {
			return [{ type: 'tool_result', data: { id: t.Error.id, content: `error: ${t.Error.error}` } }];
		}
		if (t.AutoApproved) {
			let args = t.AutoApproved.arguments;
			try { args = JSON.parse(args); } catch { /* keep as string */ }
			return [{ type: 'tool_call_auto_approved', data: { id: t.AutoApproved.id, name: t.AutoApproved.name, arguments: args } }];
		}
	}
	// User events and unknowns are dropped.
	return [];
}

/** @param {{ params: { id: string }, request: Request }} ctx */
export async function GET({ params, request }) {
	if (AGENT_URL) {
		const auth = request.headers.get('authorization') ?? '';
		const upstream = await fetch(`${AGENT_URL}/sessions/${params.id}/events`, {
			headers: { authorization: auth }
		});
		if (!upstream.ok || !upstream.body) {
			return new Response(await upstream.text(), { status: upstream.status });
		}

		const body = upstream.body;
		/** @type {ReadableStreamDefaultReader<Uint8Array>} */
		let reader;

		const stream = new ReadableStream({
			async start(controller) {
				reader = body.getReader();
				const decoder = new TextDecoder();
				let buffer = '';

				try {
					while (true) {
						const { done, value } = await reader.read();
						if (done) break;
						buffer += decoder.decode(value, { stream: true });

						let sep;
						while ((sep = buffer.indexOf('\n\n')) !== -1) {
							const frame = buffer.slice(0, sep);
							buffer = buffer.slice(sep + 2);

							const lines = frame.split('\n');
							const dataLines = [];
							for (const line of lines) {
								if (line.startsWith('data:')) dataLines.push(line.slice(5).trimStart());
							}
							if (!dataLines.length) continue;
							const raw = dataLines.join('\n');

							try {
								const serverEvent = JSON.parse(raw);
								for (const uiEvent of transformEvent(serverEvent)) {
									controller.enqueue(enc.encode(`data: ${JSON.stringify(uiEvent)}\n\n`));
								}
							} catch {
								// malformed JSON or keepalive; skip
							}
						}
					}
				} catch {
					// upstream closed or aborted
				} finally {
					controller.close();
				}
			},
			cancel() {
				reader?.cancel();
			}
		});

		return new Response(stream, {
			headers: {
				'content-type': 'text/event-stream',
				'cache-control': 'no-cache',
				connection: 'keep-alive'
			}
		});
	}

	const session = getSession(params.id);
	if (!session) error(404, 'session not found');

	const { emitter } = session;

	let cleanup = () => {};
	const stream = new ReadableStream({
		start(controller) {
			/** @param {any} ev */
			function onEvent(ev) {
				try {
					controller.enqueue(enc.encode(`data: ${JSON.stringify(ev)}\n\n`));
				} catch {
					// controller already closed
				}
			}
			emitter.on('event', onEvent);
			cleanup = () => emitter.off('event', onEvent);
		},
		cancel() {
			cleanup();
		}
	});

	return new Response(stream, {
		headers: {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache',
			connection: 'keep-alive'
		}
	});
}
