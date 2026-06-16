import { getSession } from '$lib/mock-store.js';
import { error } from '@sveltejs/kit';

/** @param {{ params: { id: string } }} ctx */
export async function POST({ params }) {
	const session = getSession(params.id);
	if (!session) error(404, 'session not found');
	return new Response(null, { status: 200 });
}
