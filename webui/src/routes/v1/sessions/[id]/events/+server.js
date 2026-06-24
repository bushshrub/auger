import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;
const enc = new TextEncoder();

/**
 * Map a server AgentEvent (externally-tagged Rust enum) to UI event shape(s).
 * Server: { "Content": { "delta": "..." } } | { "Reasoning": { "delta": "..." } } | ...
 * UI: { type: string, data: any }
 *
 * Returns an array (may be empty or multiple events, e.g. Done emits metrics + turn_complete).
 *
 * @param {any} ev
 * @returns {any[]}
 */
function transformEvent(ev) {
	if (ev.Content) return [{ type: 'content', data: { text: ev.Content.delta } }];
	if (ev.Reasoning) return [{ type: 'reasoning', data: { text: ev.Reasoning.delta } }];
	if (ev.ToolCallRequest) {
		let args = ev.ToolCallRequest.arguments;
		try { args = JSON.parse(args); } catch { /* keep as string */ }
		return [{ type: 'tool_call', data: { id: ev.ToolCallRequest.id, name: ev.ToolCallRequest.name, arguments: args } }];
	}
	if (ev.ToolCallResult) {
		return [{ type: 'tool_result', data: { id: ev.ToolCallResult.id, content: ev.ToolCallResult.result } }];
	}
	if (ev.ToolCallAutoApproved) {
		let args = ev.ToolCallAutoApproved.arguments;
		try { args = JSON.parse(args); } catch { /* keep as string */ }
		return [{ type: 'tool_call_auto_approved', data: { id: ev.ToolCallAutoApproved.id, name: ev.ToolCallAutoApproved.name, arguments: args } }];
	}
	if (ev.ToolCallDenied) {
		return [{ type: 'tool_result', data: { id: ev.ToolCallDenied.id, content: `denied: ${ev.ToolCallDenied.reason}` } }];
	}
	if (ev.Done) {
		const out = [];
		if (ev.Done.usage) {
			out.push({ type: 'metrics', data: {
				prompt_tokens: ev.Done.usage.prompt_tokens,
				completion_tokens: ev.Done.usage.completion_tokens,
				total_tokens: ev.Done.usage.total_tokens,
			}});
		}
		out.push({ type: 'turn_complete', data: {} });
		return out;
	}
	// UserMessage tracked locally by the UI; everything else dropped.
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

		/** @type {ReadableStreamDefaultReader<Uint8Array>} */
		let reader;

		const stream = new ReadableStream({
			async start(controller) {
				reader = upstream.body.getReader();
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
			controller._cleanup = () => emitter.off('event', onEvent);
		},
		cancel(controller) {
			controller._cleanup?.();
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
