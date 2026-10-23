// Per-session client state machine: folds the record snapshot and the live
// record stream into a normalized transcript (UiItem[]) for the view components.
//
// Lifecycle: start() subscribes to /events, fetches /snapshot, folds it, then
// replays the records that arrived meanwhile. On stream loss it reconnects with
// backoff and starts over from a fresh snapshot (the server does not replay).

import {
	ApiError,
	getSnapshot,
	interruptSession,
	respondToToolCall,
	sendInput,
	subscribeEvents
} from './api.js';
import { Fold, nextId } from './fold.js';

/**
 * @typedef {import('./api.js').SessionEvent} SessionEvent
 * @typedef {import('./fold.js').UiItem} UiItem
 * @typedef {import('./fold.js').UiToolCall} UiToolCall
 * @typedef {import('./fold.js').ToolCallStatus} ToolCallStatus
 */

/**
 * Client-side guess of which tools need consent, mirroring the server's
 * auto-approval policy (agent-server/src/main.rs). Only used for the initial
 * card status; tool_consent_required events are the source of truth.
 */
export const APPROVAL_REQUIRED_TOOLS = new Set(['shell', 'edit_file', 'write_file']);

/**
 * Read-only, auto-approved filesystem tools whose cards are collapsed into a
 * single summary dropdown when several run back to back. Keeps exploration
 * bursts (read/list/grep/...) from flooding the transcript. Web tools
 * (web_search, fetch_content) stay standalone since each is worth seeing.
 */
export const GROUPABLE_TOOLS = new Set(['read_file', 'list_files', 'glob', 'grep', 'todo_list']);

/** Live-only events that carry deltas; they wait for the snapshot to be folded. */
const DELTA_EVENTS = new Set(['text_delta', 'reasoning_delta', 'tool_call', 'tool_call_complete']);

export class AugerSession {
	/** @type {UiItem[]} */
	items = $state([]);
	busy = $state(false);
	connected = $state(false);
	/** @type {string | null} */
	error = $state(null);
	// Tokens currently in the model's context (latest turn's total), used for the
	// context-usage bar. Bounded by the context window -- this is NOT a cumulative
	// sum across turns (each turn re-sends the full history, so summing overcounts).
	contextTokens = $state(0);

	/**
	 * The agent's current todo list, parsed from the most recent todo_list tool
	 * result. Empty until the agent creates one. The tool returns the full list
	 * on every call, so the latest result is the current state.
	 * @type {{ id: number, title: string, status: 'pending' | 'in_progress' | 'done' }[]}
	 */
	todos = $derived.by(() => {
		/** @type {string | null} */
		let latest = null;
		for (const item of this.items) {
			if (item.kind === 'tool' && item.call.name === 'todo_list' && item.call.result) {
				latest = item.call.result;
			}
		}
		if (latest === null) return [];
		try {
			const parsed = JSON.parse(latest);
			return Array.isArray(parsed.items) ? parsed.items : [];
		} catch {
			return [];
		}
	});

	/** @type {AbortController | null} */
	#stream = null;
	#stopped = false;
	#backoffMs = 1000;
	/** @type {ReturnType<typeof setTimeout> | null} */
	#retryTimer = null;
	#fold = new Fold();
	/** Records and events that arrive before the snapshot is folded. @type {SessionEvent[] | null} */
	#buffer = null;
	/** Whether live deltas may be applied: a record newer than the snapshot has arrived. */
	#live = false;
	#snapshotSeq = -1;

	/**
	 * @param {string} sessionId
	 * @param {import('./api.js').SessionTokens} tokens
	 */
	constructor(sessionId, tokens) {
		this.sessionId = sessionId;
		this.tokens = tokens;
	}

	/** Subscribe to events, fetch the snapshot, and fold both. */
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
		this.#buffer = [];
		this.#live = false;
		// Subscribe before the snapshot, so no record falls between the two.
		this.#stream?.abort();
		this.#stream = subscribeEvents(
			this.sessionId,
			this.tokens.read,
			(event) => this.#onEvent(event),
			(err) => {
				this.connected = false;
				this.#handleConnectionError(err);
			}
		);
		let records;
		try {
			records = await getSnapshot(this.sessionId, this.tokens.read);
		} catch (err) {
			this.#stream.abort();
			this.#stream = null;
			this.#buffer = null;
			this.#handleConnectionError(err);
			return;
		}
		const fold = new Fold();
		/** @type {UiItem[]} */
		const items = [];
		for (const record of records) fold.apply(items, record);
		this.#fold = fold;
		this.items = items;
		this.#snapshotSeq = records.at(-1)?.seq ?? -1;
		this.busy = this.#hasUnresolvedTools();

		const buffered = this.#buffer ?? [];
		this.#buffer = null;
		for (const event of buffered) this.#onEvent(event);
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
		// The message shows when its turns record arrives.
		this.busy = true;
		this.error = null;
		try {
			await sendInput(this.sessionId, this.tokens.write, text);
		} catch (err) {
			this.busy = false;
			this.error = err instanceof Error ? err.message : String(err);
		}
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

	/** @param {SessionEvent} event */
	#onEvent(event) {
		if (this.#buffer) {
			if (event.type === 'record') this.#buffer.push(event);
			return;
		}
		if (event.type === 'record') {
			if (event.record.seq > this.#snapshotSeq) this.#live = true;
			this.#applyRecord(event.record);
		} else if (this.#live) {
			this.#applyEvent(event);
		}
	}

	/** @param {any} record */
	#applyRecord(record) {
		const effect = this.#fold.apply(this.items, record);
		if (!effect) return;
		switch (effect.kind) {
			case 'started':
				this.busy = true;
				break;
			case 'completed': {
				const usage = effect.usage;
				if (usage) {
					this.contextTokens =
						usage.total_tokens ?? (usage.prompt_tokens ?? 0) + (usage.completion_tokens ?? 0);
				}
				// The turn continues (tool execution, then another stream) while
				// any tool call is still awaiting consent or running.
				this.busy = this.#hasUnresolvedTools();
				break;
			}
			case 'stopped':
				this.busy = false;
				if (effect.error) this.error = effect.error;
				break;
			case 'settled':
				this.busy = this.#hasUnresolvedTools();
				break;
		}
	}

	/** @param {SessionEvent} e */
	#applyEvent(e) {
		switch (e.type) {
			case 'reasoning_delta':
			case 'text_delta': {
				this.#fold.openTail(this.items.length);
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
				this.#fold.openTail(this.items.length);
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
			case 'tool_results_withheld':
				this.busy = this.#hasUnresolvedTools();
				break;
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
