import { EventEmitter } from 'events';

/** @type {Map<string, { ownerToken: string, viewerToken: string, contextWindow: number, emitter: EventEmitter }>} */
const sessions = new Map();

export function createSession(id, ownerToken, viewerToken, contextWindow) {
	const emitter = new EventEmitter();
	emitter.setMaxListeners(20);
	sessions.set(id, { ownerToken, viewerToken, contextWindow, emitter });
}

/** @param {string} id */
export function getSession(id) {
	return sessions.get(id);
}

/** @param {string} id @param {any} event */
export function emit(id, event) {
	sessions.get(id)?.emitter.emit('event', event);
}
