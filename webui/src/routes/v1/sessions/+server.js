import { json } from '@sveltejs/kit';
import { createSession, listSessions } from '$lib/mock-store.js';
import { randomUUID } from 'crypto';

const AGENT_URL = process.env.AGENT_SERVER_URL;

export async function GET() {
	if (AGENT_URL) {
		const res = await fetch(`${AGENT_URL}/sessions`);
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		const { sessions } = await res.json();
		return json({
			sessions: /** @type {any[]} */ (sessions).map((s) => ({
				session_id: s.session_id,
				model: s.model,
				created_at: s.created_at,
				context_window: s.context_window ?? 8192,
				owner_token: s.tokens.write,
				viewer_token: s.tokens.read
			}))
		});
	}

	return json({ sessions: listSessions() });
}

export async function POST({ request }) {
	if (AGENT_URL) {
		const body = await request.json().catch(() => ({}));
		const res = await fetch(`${AGENT_URL}/sessions`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ model: body.model ?? null })
		});
		if (!res.ok) return new Response(await res.text(), { status: res.status });
		const { session_id, context_window, tokens } = await res.json();
		return json({
			session_id,
			owner_token: tokens.write,
			viewer_token: tokens.read,
			context_window: context_window ?? 8192
		});
	}

	const id = randomUUID();
	const ownerToken = randomUUID();
	const viewerToken = randomUUID();
	const contextWindow = 8192;
	createSession(id, ownerToken, viewerToken, contextWindow);
	return json({ session_id: id, owner_token: ownerToken, viewer_token: viewerToken, context_window: contextWindow });
}
