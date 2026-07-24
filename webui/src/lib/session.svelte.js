// Per-session client state machine: consumes the trace snapshot + SSE event stream
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
 * @typedef {import('./api.js').TraceRecord} TraceRecord
 * @typedef {import('./api.js').TokenUsage} TokenUsage
 *
 * @typedef {'pending_approval' | 'running' | 'done' | 'error' | 'denied'} ToolCallStatus
 * @typedef {{ id: string, name: string, arguments: string, status: ToolCallStatus,
 *             autoApproved: boolean, result?: string, error?: string }} UiToolCall
 * @typedef {(
 *   | { kind: 'user', id: string, text: string }
 *   | { kind: 'assistant', id: string, reasoning: string, content: string,
 *       streaming: boolean, stopReason?: string | null, empty?: boolean }
 *   | { kind: 'tool', id: string, call: UiToolCall }
 * )} UiItem
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

let counter = 0;
const nextId = () => `ui_${++counter}`;

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
	// True while sendInput is in-flight; prevents #connect from overwriting items
	// with a snapshot that predates the message the user just sent.
	#sendInFlight = false;

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
			const records = await getSnapshot(this.sessionId, this.tokens.read);
			// Skip rebuild if a send is in-flight: the snapshot may predate the
			// message the user just sent, which would erase the optimistic push.
			if (!this.#sendInFlight) {
				this.items = buildItems(records);
				this.busy = this.#hasUnresolvedTools();
			}
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
		// Push optimistically so the message appears immediately, before the
		// network round-trip. #sendInFlight prevents a concurrent #connect from
		// overwriting items with a snapshot that predates this message.
		const item = /** @type {UiItem} */ ({ kind: 'user', id: nextId(), text });
		this.items.push(item);
		this.busy = true;
		this.error = null;
		this.#sendInFlight = true;
		try {
			await sendInput(this.sessionId, this.tokens.write, text);
		} catch (err) {
			const idx = this.items.indexOf(item);
			if (idx !== -1) this.items.splice(idx, 1);
			this.busy = false;
			this.error = err instanceof Error ? err.message : String(err);
		} finally {
			this.#sendInFlight = false;
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
				// Denial is decided locally (respond) and wins over a late result,
				// which for a denied call just restates the denial reason.
				if (call && call.status !== 'denied') applyOutcome(call, e.result.outcome);
				break;
			}
			case 'done': {
				const existing = this.#lastStreaming();
				if (existing) {
					existing.streaming = false;
					existing.stopReason = e.stop_reason;
				}
				if (e.usage) {
					this.contextTokens =
						e.usage.total_tokens ??
						(e.usage.prompt_tokens ?? 0) + (e.usage.completion_tokens ?? 0);
				}
				// Surface turns that completed without any assistant output (no
				// reasoning, no content) and without calling a tool -- otherwise the
				// turn is invisible. Happens when a model tier streams zero deltas.
				// stop_reason 'tool_calls' means tools ran, so it isn't empty.
				const noOutput =
					!existing || (existing.reasoning.trim() === '' && existing.content.trim() === '');
				if (noOutput && e.stop_reason !== 'tool_calls' && !this.#hasUnresolvedTools()) {
					if (existing) {
						existing.empty = true;
					} else {
						this.items.push({
							kind: 'assistant',
							id: nextId(),
							reasoning: '',
							content: '',
							streaming: false,
							stopReason: e.stop_reason,
							empty: true
						});
					}
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
 * Rebuild the transcript from a trace snapshot (initial load and reconnect).
 * The snapshot is the persisted record stream (see FE.md): a leading session
 * record, then turns (which carry the conversation), then the tool events that
 * occurred during an assistant turn. Turns and their enum payloads are
 * externally tagged (`turn.input_message`, `turn.assistant_message.outcome`,
 * `event.tool_authorization`, ...). Assistant turns also emit a
 * `tool_call_requested` event, but the card is already created from the turn's
 * response `tool_calls`, so that event is ignored here.
 * @param {TraceRecord[]} records
 * @returns {UiItem[]}
 */
function buildItems(records) {
	/** @type {UiItem[]} */
	const items = [];
	/** @type {Map<string, UiToolCall>} */
	const calls = new Map();

	for (const record of records) {
		if (record.kind === 'session') continue;

		if (record.kind === 'turn' && 'input_message' in record.turn) {
			for (const content of record.turn.input_message.content) {
				if (content.type === 'text' && content.text.trim().length > 0) {
					items.push({ kind: 'user', id: nextId(), text: content.text });
				} else if (content.type === 'tool_result') {
					const call = calls.get(content.tool_call_id);
					if (call && call.status !== 'denied') {
						call.status = 'done';
						call.result = toolText(content.content);
					}
				}
			}
		} else if (record.kind === 'turn' && 'assistant_message' in record.turn) {
			// Only completed / interrupted outcomes carry a response; failed has none.
			const outcome = record.turn.assistant_message.outcome;
			const response =
				outcome === 'failed'
					? null
					: 'completed' in outcome
						? outcome.completed.response
						: outcome.interrupted.partial_response;
			if (!response) continue;

			const reasoning = response.reasoning ?? '';
			const text = response.content;
			if (reasoning.trim().length > 0 || text.trim().length > 0) {
				items.push({
					kind: 'assistant',
					id: nextId(),
					reasoning,
					content: text,
					streaming: false
				});
			}
			for (const tc of response.tool_calls) {
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
		} else if (record.kind === 'event' && 'tool_authorization' in record.event) {
			// A turn's events can arrive in any order, so only advance the status;
			// never downgrade a call that already has a result back to 'running'.
			const auth = record.event.tool_authorization;
			const call = calls.get(auth.tool_call_id);
			if (call) {
				call.autoApproved = auth.source === 'policy';
				if (auth.decision === 'denied') call.status = 'denied';
				else if (call.status === 'pending_approval') call.status = 'running';
			}
		} else if (record.kind === 'event' && 'tool_call_result' in record.event) {
			const result = record.event.tool_call_result;
			const call = calls.get(result.tool_call_id);
			// Denial wins over a result (a denied call still records its reason
			// as a result), so don't overwrite that status.
			if (call && call.status !== 'denied') applyOutcome(call, result.outcome);
		}
	}

	return items;
}

/**
 * Apply a tool-call outcome to its card, setting status plus result/error text.
 * @param {UiToolCall} call
 * @param {import('./api.js').ToolOutcome} outcome
 */
function applyOutcome(call, outcome) {
	if (outcome === 'interrupted') {
		call.status = 'error';
		call.error = 'Tool call was interrupted.';
	} else if ('success' in outcome) {
		call.status = 'done';
		call.result = toolText(outcome.success.content);
	} else if ('error' in outcome) {
		call.status = 'error';
		call.error = toolText(outcome.error.error);
	} else if ('denied' in outcome) {
		call.status = 'denied';
		call.error = outcome.denied.reason ?? 'Tool call was denied.';
	}
}

/** @param {import('./api.js').ToolData[]} content */
function toolText(content) {
	return content.map((item) => item.text.text).join('');
}
