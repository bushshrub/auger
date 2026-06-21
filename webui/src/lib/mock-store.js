import { EventEmitter } from 'events';

/** @type {Map<string, { ownerToken: string, viewerToken: string, contextWindow: number, model: string, createdAt: number, emitter: EventEmitter }>} */
const sessions = new Map();

/**
 * @param {string} id
 * @param {string} ownerToken
 * @param {string} viewerToken
 * @param {number} contextWindow
 * @param {string} [model]
 */
export function createSession(id, ownerToken, viewerToken, contextWindow, model = 'mock') {
	const emitter = new EventEmitter();
	emitter.setMaxListeners(20);
	sessions.set(id, { ownerToken, viewerToken, contextWindow, model, createdAt: Math.floor(Date.now() / 1000), emitter });
}

/** @param {string} id */
export function getSession(id) {
	return sessions.get(id);
}

export function listSessions() {
	return Array.from(sessions.entries()).map(([id, s]) => ({
		session_id: id,
		model: s.model,
		created_at: s.createdAt,
		context_window: s.contextWindow,
		owner_token: s.ownerToken,
		viewer_token: s.viewerToken
	}));
}

/** @param {string} id @param {any} event */
export function emit(id, event) {
	sessions.get(id)?.emitter.emit('event', event);
}
