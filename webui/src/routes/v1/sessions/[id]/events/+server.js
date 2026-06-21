import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;
const enc = new TextEncoder();

/**
 * Map a server AgentEvent (externally-tagged Rust enum) to the UI event shape.
 * Server: { "Content": { "delta": "..." } } | { "Reasoning": { "delta": "..." } } | { "UserMessage": { ... } }
 * UI: { type: string, data: any }
 *
 * @param {any} ev
 * @returns {any|null}
 */
function transformEvent(ev) {
	if (ev === 'Done') return { type: 'turn_complete', data: {} };
	if (ev.Content) return { type: 'content', data: { text: ev.Content.delta } };
	// Reasoning is not yet rendered in the UI; map it to content so text isn't lost.
	if (ev.Reasoning) return { type: 'content', data: { text: ev.Reasoning.delta } };
	// UserMessage is tracked locally by the UI; drop it.
	return null;
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
								const uiEvent = transformEvent(serverEvent);
								if (uiEvent) {
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
