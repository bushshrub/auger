// Sidebar activity tracker. The server does not report per-session status, but a
// live session emits SSE events while it works and goes quiet when it parks for
// input. We subscribe to each non-archived session's event stream (subscribing
// does not activate a dormant session) and derive a coarse working/idle signal.
//
// Limitation: a never-started session looks identical to an idle one over the
// API, so both read as 'idle'.

import { subscribeEvents } from './api.js';

/** @typedef {'idle' | 'working'} SessionStatus */

/**
 * Tracks whether a single session is mid-turn. Working means the model is
 * streaming or tool calls are still outstanding (running or awaiting consent).
 */
class Monitor {
	#streaming = false;
	/** @type {Set<string>} */
	#pending = new Set();
	/** @type {() => void} */
	#onchange;

	/** @param {() => void} onchange */
	constructor(onchange) {
		this.#onchange = onchange;
	}

	get working() {
		return this.#streaming || this.#pending.size > 0;
	}

	/** @param {import('./api.js').SessionEvent} e */
	apply(e) {
		switch (e.type) {
			case 'text_delta':
			case 'reasoning_delta':
				this.#streaming = true;
				break;
			case 'tool_call_complete':
				this.#pending.add(e.id);
				this.#streaming = false;
				break;
			case 'tool_consent_required':
				for (const tc of e.tool_calls) this.#pending.add(tc.id);
				this.#streaming = false;
				break;
			case 'tool_call_result':
				this.#pending.delete(e.id);
				break;
			case 'done':
				this.#streaming = false;
				break;
			case 'interrupted':
			case 'stream_error':
			case 'closed':
				this.#streaming = false;
				this.#pending.clear();
				break;
			default:
				return;
		}
		this.#onchange();
	}

	reset() {
		this.#streaming = false;
		this.#pending.clear();
	}
}

/**
 * Maintains a live status map for a set of sessions. Call sync() whenever the
 * session list changes; it opens streams for new sessions and drops archived or
 * removed ones. Reactive: read `statuses[sessionId]` from a component.
 */
export class StatusTracker {
	/** @type {Record<string, SessionStatus>} */
	statuses = $state({});

	/** @type {Map<string, { controller: AbortController, monitor: Monitor,
	 *   token: string, retry: ReturnType<typeof setTimeout> | null }>} */
	#subs = new Map();
	#stopped = false;

	/** @param {import('./api.js').SessionInfo[]} sessions */
	sync(sessions) {
		if (this.#stopped) return;
		const want = new Set(sessions.filter((s) => !s.archived).map((s) => s.session_id));
		for (const [id, sub] of this.#subs) {
			if (!want.has(id)) {
				sub.controller.abort();
				if (sub.retry !== null) clearTimeout(sub.retry);
				this.#subs.delete(id);
				delete this.statuses[id];
			}
		}
		for (const s of sessions) {
			if (s.archived || this.#subs.has(s.session_id)) continue;
			this.#open(s.session_id, s.tokens.read);
		}
	}

	/**
	 * @param {string} id
	 * @param {string} token
	 */
	#open(id, token) {
		const monitor = new Monitor(() => {
			this.statuses[id] = monitor.working ? 'working' : 'idle';
		});
		if (!(id in this.statuses)) this.statuses[id] = 'idle';
		const entry = {
			monitor,
			token,
			retry: /** @type {ReturnType<typeof setTimeout> | null} */ (null),
			controller: subscribeEvents(
				id,
				token,
				(e) => monitor.apply(e),
				() => this.#onClose(id)
			)
		};
		this.#subs.set(id, entry);
	}

	/** @param {string} id */
	#onClose(id) {
		const entry = this.#subs.get(id);
		if (!entry || this.#stopped) return;
		// Lost the stream; treat as idle and reconnect after a short delay.
		entry.monitor.reset();
		this.statuses[id] = 'idle';
		entry.retry = setTimeout(() => {
			if (this.#stopped || !this.#subs.has(id)) return;
			entry.controller = subscribeEvents(
				id,
				entry.token,
				(e) => entry.monitor.apply(e),
				() => this.#onClose(id)
			);
		}, 3000);
	}

	stop() {
		this.#stopped = true;
		for (const sub of this.#subs.values()) {
			sub.controller.abort();
			if (sub.retry !== null) clearTimeout(sub.retry);
		}
		this.#subs.clear();
	}
}
