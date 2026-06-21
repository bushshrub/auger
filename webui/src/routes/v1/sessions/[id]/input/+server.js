import { getSession, emit } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;

const MOCK_WORDS = 'This is a mock response from the auger dev server. The real agent-server is not running — set AGENT_SERVER_URL to connect to it.'.split(' ');

/** @param {{ params: { id: string }, request: Request }} ctx */
export async function POST({ params, request }) {
	if (AGENT_URL) {
		const auth = request.headers.get('authorization') ?? '';
		const body = await request.json();
		const res = await fetch(`${AGENT_URL}/sessions/${params.id}/input`, {
			method: 'POST',
			headers: {
				'content-type': 'application/json',
				authorization: auth
			},
			// server expects `input`, client sends `content`
			body: JSON.stringify({ input: body.content ?? body.input ?? '' })
		});
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		return new Response(null, { status: 202 });
	}

	const session = getSession(params.id);
	if (!session) error(404, 'session not found');

	const body = await request.json();
	const userText = body.content ?? '';

	const doTool = /\bfile\b|\bread\b/i.test(userText);

	if (doTool) {
		const toolId = crypto.randomUUID();
		setTimeout(() => {
			emit(params.id, { type: 'tool_call', data: { id: toolId, name: 'read_file', arguments: { path: '/tmp/example.txt' } } });
		}, 100);

		setTimeout(() => {
			emit(params.id, { type: 'tool_result', data: { id: toolId, content: '1\thello world\n2\tfoo bar\n' } });
		}, 600);
	}

	const startDelay = doTool ? 800 : 50;
	let delay = startDelay;

	for (const word of MOCK_WORDS) {
		const w = word;
		setTimeout(() => emit(params.id, { type: 'content', data: { text: w + ' ' } }), delay);
		delay += 60;
	}

	const total = MOCK_WORDS.length;
	setTimeout(() => emit(params.id, { type: 'metrics', data: { completion_tokens: total, tokens_per_sec: 16.0, ttft_ms: startDelay } }), delay + 10);
	setTimeout(() => emit(params.id, { type: 'turn_complete', data: {} }), delay + 20);

	return new Response(null, { status: 202 });
}
