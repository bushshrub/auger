import { getSession } from '$lib/mock-store.js';
import { error, json } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;

/** @param {{ params: { id: string }, request: Request }} ctx */
export async function GET({ params, request }) {
	if (AGENT_URL) {
		const auth = request.headers.get('authorization') ?? '';
		const res = await fetch(`${AGENT_URL}/sessions/${params.id}/snapshot`, {
			headers: { authorization: auth }
		});
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		return json(await res.json());
	}

	const session = getSession(params.id);
	if (!session) error(404, 'session not found');
	return json({ messages: [] });
}
