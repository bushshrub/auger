import { json } from '@sveltejs/kit';
import { createSession } from '$lib/mock-store.js';
import { randomUUID } from 'crypto';

export async function POST() {
	const id = randomUUID();
	const ownerToken = randomUUID();
	const viewerToken = randomUUID();
	const contextWindow = 8192;

	createSession(id, ownerToken, viewerToken, contextWindow);

	return json({ session_id: id, owner_token: ownerToken, viewer_token: viewerToken, context_window: contextWindow });
}
