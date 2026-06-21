import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

const AGENT_URL = process.env.AGENT_SERVER_URL;

/** @param {{ params: { id: string }, request: Request }} ctx */
export async function POST({ params, request }) {
	if (AGENT_URL) {
		const auth = request.headers.get('authorization') ?? '';
		const body = await request.json();
		// server uses /tool, not /approve
		const res = await fetch(`${AGENT_URL}/sessions/${params.id}/tool`, {
			method: 'POST',
			headers: {
				'content-type': 'application/json',
				authorization: auth
			},
			body: JSON.stringify(body)
		});
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		return new Response(null, { status: 200 });
	}

	const session = getSession(params.id);
	if (!session) error(404, 'session not found');
	return new Response(null, { status: 200 });
}
