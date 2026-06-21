import { json } from '@sveltejs/kit';
import { createSession } from '$lib/mock-store.js';
import { randomUUID } from 'crypto';

const AGENT_URL = process.env.AGENT_SERVER_URL;

export async function POST({ request }) {
	if (AGENT_URL) {
		const body = await request.json().catch(() => ({}));
		const res = await fetch(`${AGENT_URL}/sessions`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ model: body.model ?? null })
		});
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		const { session_id, tokens } = await res.json();
		return json({
			session_id,
			owner_token: tokens.write,
			viewer_token: tokens.read,
			context_window: 8192
		});
	}

	const id = randomUUID();
	const ownerToken = randomUUID();
	const viewerToken = randomUUID();
	const contextWindow = 8192;
	createSession(id, ownerToken, viewerToken, contextWindow);
	return json({ session_id: id, owner_token: ownerToken, viewer_token: viewerToken, context_window: contextWindow });
}
