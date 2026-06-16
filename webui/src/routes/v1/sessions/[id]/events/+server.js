import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const enc = new TextEncoder();

/** @param {{ params: { id: string } }} ctx */
export async function GET({ params }) {
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

			// stash cleanup on the controller so cancel() can reach it
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
