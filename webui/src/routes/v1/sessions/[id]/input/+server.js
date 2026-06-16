import { getSession, emit } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const MOCK_WORDS = 'This is a mock response from the auger dev server. The real agent-server is not running — set AGENT_SERVER_URL to connect to it.'.split(' ');

/** @param {{ params: { id: string }, request: Request }} ctx */
export async function POST({ params, request }) {
	const session = getSession(params.id);
	if (!session) error(404, 'session not found');

	const body = await request.json();
	const userText = body.content ?? '';

	// Optionally demo a tool call if the user mentions "file" or "read".
	const doTool = /\bfile\b|\bread\b/i.test(userText);

	if (doTool) {
		const toolId = crypto.randomUUID();
		setTimeout(() => {
			emit(params.id, { type: 'tool_call', data: { id: toolId, name: 'read_file', arguments: { path: '/tmp/example.txt' } } });
		}, 100);

		// auto-approve after 500 ms so the demo doesn't stall
		setTimeout(() => {
			emit(params.id, { type: 'tool_result', data: { id: toolId, content: '1\thello world\n2\tfoo bar\n' } });
		}, 600);
	}

	const startDelay = doTool ? 800 : 50;
	let delay = startDelay;
	const words = MOCK_WORDS;

	for (const word of words) {
		const w = word;
		setTimeout(() => emit(params.id, { type: 'content', data: { text: w + ' ' } }), delay);
		delay += 60;
	}

	const total = words.length;
	setTimeout(() => emit(params.id, { type: 'metrics', data: { completion_tokens: total, tokens_per_sec: 16.0, ttft_ms: startDelay } }), delay + 10);
	setTimeout(() => emit(params.id, { type: 'turn_complete', data: {} }), delay + 20);

	return new Response(null, { status: 202 });
}
