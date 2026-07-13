// Per-session client state machine: consumes the snapshot + SSE event stream
// and maintains a normalized transcript (UiItem[]) for the view components.
//
// Lifecycle: start() fetches /snapshot, rebuilds the transcript, then
// subscribes to /events. On stream loss it reconnects with backoff and
// rebuilds from a fresh snapshot (the server does not replay events).

import {
	ApiError,
	getSnapshot,
	interruptSession,
	respondToToolCall,
	sendInput,
	subscribeEvents
} from './api.js';

/**
 * @typedef {import('./api.js').SessionEvent} SessionEvent
 * @typedef {import('./api.js').SnapshotMessage} SnapshotMessage
 * @typedef {import('./api.js').TokenUsage} TokenUsage
 *
 * @typedef {'pending_approval' | 'running' | 'done' | 'error' | 'denied'} ToolCallStatus
 * @typedef {{ id: string, name: string, arguments: string, status: ToolCallStatus,
 *             autoApproved: boolean, result?: string, error?: string }} UiToolCall
 * @typedef {(
 *   | { kind: 'user', id: string, text: string }
 *   | { kind: 'assistant', id: string, reasoning: string, content: string,
 *       streaming: boolean, stopReason?: string | null }
 *   | { kind: 'tool', id: string, call: UiToolCall }
 * )} UiItem
 */

/**
 * Client-side guess of which tools need consent, mirroring the server's
 * auto-approval policy (agent-server/src/main.rs). Only used for the initial
 * card status; tool_consent_required events are the source of truth.
 */
export const APPROVAL_REQUIRED_TOOLS = new Set(['shell', 'edit_file', 'write_file']);

let counter = 0;
const nextId = () => `ui_${++counter}`;

export class AugerSession {
	/** @type {UiItem[]} */
	items = $state([]);
	busy = $state(false);
	connected = $state(false);
	/** @type {string | null} */
	error = $state(null);
	totalUsage = $state({ prompt_tokens: 0, completion_tokens: 0 });
	contextTokens = $state(0);

	/** @type {AbortController | null} */
	#stream = null;
	#stopped = false;
	#backoffMs = 1000;
	/** @type {ReturnType<typeof setTimeout> | null} */
	#retryTimer = null;

	/**
	 * @param {string} sessionId
	 * @param {import('./api.js').SessionTokens} tokens
	 */
	constructor(sessionId, tokens) {
		this.sessionId = sessionId;
		this.tokens = tokens;
	}

	/** Fetch the snapshot, rebuild the transcript, and subscribe to events. */
	async start() {
		this.#stopped = false;
		await this.#connect();
	}

	/** Close the stream and stop reconnecting. */
	stop() {
		this.#stopped = true;
		if (this.#retryTimer !== null) clearTimeout(this.#retryTimer);
		this.#stream?.abort();
		this.#stream = null;
		this.connected = false;
	}

	async #connect() {
		try {
			const { messages } = await getSnapshot(this.sessionId, this.tokens.read);
			this.items = buildItems(messages);
			this.busy = this.#hasUnresolvedTools();
		} catch (err) {
			this.#handleConnectionError(err);
			return;
		}

		this.#stream = subscribeEvents(
			this.sessionId,
			this.tokens.read,
			(event) => this.#applyEvent(event),
			(err) => {
				this.connected = false;
				this.#handleConnectionError(err);
			}
		);
		this.connected = true;
		this.error = null;
		this.#backoffMs = 1000;
	}

	/** @param {unknown} err */
	#handleConnectionError(err) {
		if (this.#stopped) return;
		if (err instanceof ApiError && (err.status === 404 || err.status === 401)) {
			// Session was deleted or the token is bad; reconnecting won't help.
			this.error = err.status === 404 ? 'session no longer exists' : err.message;
			this.stop();
			return;
		}
		this.error = err instanceof Error ? err.message : String(err);
		this.#retryTimer = setTimeout(() => this.#connect(), this.#backoffMs);
		this.#backoffMs = Math.min(this.#backoffMs * 2, 10_000);
	}

	/** @param {string} text */
	async send(text) {
		await sendInput(this.sessionId, this.tokens.write, text);
		// The server does not echo user messages on the event stream.
		this.items.push({ kind: 'user', id: nextId(), text });
		this.busy = true;
		this.error = null;
	}

	/**
	 * Approve or deny a pending tool call, flipping the card optimistically
	 * (the wire has no decision-acknowledged event).
	 * @param {string} toolCallId
	 * @param {boolean} approved
	 * @param {string} [message]
	 */
	async respond(toolCallId, approved, message) {
		await respondToToolCall(this.sessionId, this.tokens.write, toolCallId, approved, message);
		const call = this.#findCall(toolCallId);
		if (call) call.status = approved ? 'running' : 'denied';
	}

	/**
	 * Stop the in-flight turn. While tool calls are awaiting consent the
	 * server ignores interrupts, so "stop" there means denying every pending
	 * call; otherwise interrupt the active stream / tool execution. The two
	 * states are mutually exclusive (nothing streams during consent).
	 */
	async interrupt() {
		const pending = this.items.flatMap((i) =>
			i.kind === 'tool' && i.call.status === 'pending_approval' ? [i.call.id] : []
		);
		try {
			if (pending.length > 0) {
				await Promise.all(pending.map((id) => this.respond(id, false)));
			} else {
				await interruptSession(this.sessionId, this.tokens.write);
			}
		} catch (err) {
			this.error = err instanceof Error ? err.message : String(err);
		}
	}

	/** @param {string} id */
	#findCall(id) {
		for (const item of this.items) {
			if (item.kind === 'tool' && item.call.id === id) return item.call;
		}
		return null;
	}

	/** The last item, if it is an assistant bubble that is still streaming. */
	#lastStreaming() {
		const last = this.items[this.items.length - 1];
		return last?.kind === 'assistant' && last.streaming ? last : null;
	}

	#closeStreamingBubble() {
		const existing = this.#lastStreaming();
		if (existing) existing.streaming = false;
	}

	#hasUnresolvedTools() {
		return this.items.some(
			(i) => i.kind === 'tool' && (i.call.status === 'pending_approval' || i.call.status === 'running')
		);
	}

	/**
	 * @param {string} id
	 * @param {string} name
	 * @param {string} args
	 * @param {ToolCallStatus} status
	 * @param {boolean} autoApproved
	 */
	#upsertCall(id, name, args, status, autoApproved) {
		const call = this.#findCall(id);
		if (call) {
			call.name = name;
			call.arguments = args;
			call.status = status;
			call.autoApproved = autoApproved;
		} else {
			this.items.push({
				kind: 'tool',
				id: nextId(),
				call: { id, name, arguments: args, status, autoApproved }
			});
		}
	}

	/** @param {SessionEvent} e */
	#applyEvent(e) {
		switch (e.type) {
			case 'reasoning_delta':
			case 'text_delta': {
				const existing = this.#lastStreaming();
				if (existing) {
					if (e.type === 'reasoning_delta') existing.reasoning += e.text;
					else existing.content += e.text;
				} else {
					this.items.push({
						kind: 'assistant',
						id: nextId(),
						reasoning: e.type === 'reasoning_delta' ? e.text : '',
						content: e.type === 'text_delta' ? e.text : '',
						streaming: true
					});
				}
				this.busy = true;
				break;
			}
			case 'tool_call':
				// Incremental argument deltas; wait for tool_call_complete.
				break;
			case 'tool_call_complete': {
				// Close the current bubble so post-tool content starts fresh.
				this.#closeStreamingBubble();
				if (!this.#findCall(e.id)) {
					const needsApproval = APPROVAL_REQUIRED_TOOLS.has(e.name);
					this.#upsertCall(
						e.id,
						e.name,
						e.arguments,
						needsApproval ? 'pending_approval' : 'running',
						!needsApproval
					);
				}
				this.busy = true;
				break;
			}
			case 'tool_consent_required': {
				this.#closeStreamingBubble();
				for (const tc of e.tool_calls) {
					this.#upsertCall(tc.id, tc.name, tc.arguments, 'pending_approval', false);
				}
				this.busy = true;
				break;
			}
			case 'tool_call_result': {
				const call = this.#findCall(e.id);
				if (call) {
					call.status = 'done';
					call.result = e.result;
				}
				break;
			}
			case 'tool_call_error': {
				const call = this.#findCall(e.id);
				if (call) {
					if (call.status !== 'denied') call.status = 'error';
					call.error = e.error;
				}
				break;
			}
			case 'done': {
				const existing = this.#lastStreaming();
				if (existing) {
					existing.streaming = false;
					existing.stopReason = e.stop_reason;
				}
				if (e.usage) {
					this.totalUsage = {
						prompt_tokens: this.totalUsage.prompt_tokens + (e.usage.prompt_tokens ?? 0),
						completion_tokens:
							this.totalUsage.completion_tokens + (e.usage.completion_tokens ?? 0)
					};
					this.contextTokens =
						e.usage.total_tokens ??
						(e.usage.prompt_tokens ?? 0) + (e.usage.completion_tokens ?? 0);
				}
				// The turn continues (tool execution, then another stream) while
				// any tool call is still awaiting consent or running.
				this.busy = this.#hasUnresolvedTools();
				break;
			}
			case 'interrupted': {
				// Stream cut short; the partial response is committed server-side.
				this.#closeStreamingBubble();
				this.busy = false;
				break;
			}
			case 'stream_error': {
				this.#closeStreamingBubble();
				this.busy = false;
				this.error = e.error;
				break;
			}
			case 'closed': {
				this.busy = false;
				this.error = 'session closed';
				this.stop();
				break;
			}
			default:
				// Unknown event types are ignored for forward compatibility.
				break;
		}
	}
}

/**
 * Rebuild the transcript from a snapshot (initial load and reconnect).
 * Tool results ride as separate `tool` messages that complete earlier cards;
 * trailing calls without results are still in flight or awaiting consent.
 * @param {SnapshotMessage[]} messages
 * @returns {UiItem[]}
 */
function buildItems(messages) {
	/** @type {UiItem[]} */
	const items = [];
	/** @type {Map<string, UiToolCall>} */
	const calls = new Map();

	for (const m of messages) {
		if (m.type === 'user') {
			// Tool-results-only user messages have no text; skip the empty line.
			if (m.text.trim().length > 0) {
				items.push({ kind: 'user', id: nextId(), text: m.text });
			}
		} else if (m.type === 'assistant') {
			items.push({
				kind: 'assistant',
				id: nextId(),
				reasoning: m.reasoning ?? '',
				content: m.content,
				streaming: false
			});
			for (const tc of m.tool_calls) {
				const needsApproval = APPROVAL_REQUIRED_TOOLS.has(tc.name);
				/** @type {UiToolCall} */
				const call = {
					id: tc.id,
					name: tc.name,
					arguments: tc.arguments,
					status: needsApproval ? 'pending_approval' : 'running',
					autoApproved: !needsApproval
				};
				calls.set(tc.id, call);
				items.push({ kind: 'tool', id: nextId(), call });
			}
		} else if (m.type === 'tool') {
			const call = calls.get(m.tool_call_id);
			if (call) {
				call.status = 'done';
				call.result = m.content;
			}
		}
	}

	return items;
}
