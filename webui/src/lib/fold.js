// Folds session records into the transcript (UiItem[]). Snapshot records and
// live `record` events take the same path, so the transcript does not depend
// on how it was loaded. Streamed deltas are provisional: they sit after the
// settled items until the next record, which replaces them.

/**
 * @typedef {import('./api.js').ToolOutcome} ToolOutcome
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
 *
 * What a record means for the rest of the client, if anything.
 * @typedef {(
 *   | { kind: 'started' }
 *   | { kind: 'completed', usage: import('./api.js').TokenUsage | null }
 *   | { kind: 'stopped', error: string | null }
 *   | { kind: 'settled' }
 * )} Effect
 */

let counter = 0;
/** @returns {string} */
export const nextId = () => `ui_${++counter}`;

export class Fold {
	/** @type {number} */
	lastSeq = -1;
	/** Items each driver turn added, in order. `truncate` drops the later ones. @type {number[]} */
	#turnItems = [];
	/** Where the partial response of an interrupted or failed stream starts. @type {number | null} */
	#partial = null;
	/** Where streamed items start; the next record replaces them. @type {number | null} */
	#tail = null;
	/** Whether the last output turn showed anything (text, reasoning or a call). */
	#lastOutputVisible = false;

	/** Mark `len` as the start of live items, unless already marked. @param {number} len */
	openTail(len) {
		this.#tail ??= len;
	}

	/**
	 * Fold one record into `items`, in place.
	 * @param {UiItem[]} items
	 * @param {any} record
	 * @returns {Effect | null | undefined} undefined when the record was already folded
	 */
	apply(items, record) {
		if (typeof record?.seq !== 'number' || record.seq <= this.lastSeq) return undefined;
		this.lastSeq = record.seq;
		if (this.#tail !== null) {
			items.splice(this.#tail);
			this.#tail = null;
		}
		const data = record.data;
		switch (record.type) {
			case 'turns':
				this.#dropPartial(items);
				this.#turns(items, data);
				return null;
			case 'stream':
				return this.#stream(items, data);
			case 'tool':
				return this.#tool(items, data);
			default:
				return null;
		}
	}

	/** @param {UiItem[]} items */
	#dropPartial(items) {
		if (this.#partial !== null) {
			items.splice(this.#partial);
			this.#partial = null;
		}
	}

	/** @param {UiItem[]} items @param {any} data */
	#turns(items, data) {
		if ('push_turn' in data) {
			const turn = data.push_turn;
			const start = items.length;
			if ('input' in turn) {
				for (const entry of turn.input.entries) this.#pushEntry(items, entry);
				this.#turnItems.push(items.length - start);
			} else {
				pushResponse(items, turn.output, false);
				this.#lastOutputVisible = items.length > start;
				this.#turnItems.push(items.length - start);
			}
		} else if ('push_entry' in data) {
			const before = items.length;
			this.#pushEntry(items, data.push_entry);
			const last = this.#turnItems.length - 1;
			if (last >= 0) this.#turnItems[last] += items.length - before;
		} else if ('truncate' in data) {
			const len = Math.min(data.truncate.len, this.#turnItems.length);
			items.splice(this.#turnItems.slice(0, len).reduce((sum, n) => sum + n, 0));
			this.#turnItems.length = len;
		}
	}

	/** @param {UiItem[]} items @param {any} entry */
	#pushEntry(items, entry) {
		// Harness entries (tool results, messages) show through their tool calls.
		if ('user' in entry && entry.user.message.trim().length > 0) {
			items.push({ kind: 'user', id: nextId(), text: entry.user.message });
		}
	}

	/** @param {UiItem[]} items @param {any} data @returns {Effect | null} */
	#stream(items, data) {
		if (data === 'started') {
			this.#dropPartial(items);
			return { kind: 'started' };
		}
		if (typeof data !== 'object' || data === null) return null;
		if ('completed' in data) {
			const { usage, stop_reason } = data.completed;
			const last = items.at(-1);
			if (this.#lastOutputVisible && last?.kind === 'assistant') {
				last.stopReason = stop_reason;
			}
			// A turn that completed with no output and no tool call is invisible
			// otherwise. Happens when a model tier streams zero deltas.
			if (!this.#lastOutputVisible && stop_reason !== 'tool_calls') {
				items.push({
					kind: 'assistant',
					id: nextId(),
					reasoning: '',
					content: '',
					streaming: false,
					stopReason: stop_reason,
					empty: true
				});
			}
			return { kind: 'completed', usage };
		}
		if ('interrupted' in data || 'failed' in data) {
			const body = data.interrupted ?? data.failed;
			this.#dropPartial(items);
			const start = items.length;
			if (Array.isArray(body.partial)) pushResponse(items, body.partial, true);
			this.#partial = start;
			return { kind: 'stopped', error: body.error?.message ?? null };
		}
		return null;
	}

	/** @param {UiItem[]} items @param {any} data @returns {Effect | null} */
	#tool(items, data) {
		if ('decided' in data) {
			const { tool_call_id: id, decision } = data.decided;
			const call = findCall(items, id);
			if (call) {
				if ('allowed' in decision) {
					call.status = 'running';
					call.autoApproved = decision.allowed.by !== 'user';
				} else {
					call.status = 'denied';
				}
			}
			return { kind: 'settled' };
		}
		if ('finished' in data) {
			const call = findCall(items, data.finished.tool_call_id);
			if (call) applyOutcome(call, data.finished.outcome);
			return { kind: 'settled' };
		}
		return null;
	}
}

/** @param {UiItem[]} items @param {string} id */
function findCall(items, id) {
	for (const item of items) {
		if (item.kind === 'tool' && item.call.id === id) return item.call;
	}
	return null;
}

/**
 * Push the items of a response. Calls of a partial response never ran, so
 * they show as interrupted.
 * @param {UiItem[]} items
 * @param {any[]} blocks
 * @param {boolean} partial
 */
function pushResponse(items, blocks, partial) {
	const { reasoning, content: text, toolCalls } = responseParts({ blocks });
	if (reasoning.trim().length > 0 || text.trim().length > 0) {
		items.push({ kind: 'assistant', id: nextId(), reasoning, content: text, streaming: false });
	}
	for (const tc of toolCalls) {
		/** @type {UiToolCall} */
		const call = {
			id: tc.id,
			name: tc.name,
			arguments: tc.arguments,
			status: 'pending_approval',
			autoApproved: false
		};
		if (partial) applyOutcome(call, 'interrupted');
		items.push({ kind: 'tool', id: nextId(), call });
	}
}

/**
 * Normalize persisted provider blocks for the transcript. Uppercase keys keep
 * snapshots written before the provider tags became snake_case readable.
 * @param {import('./api.js').AssistantResponse | any} response
 */
function responseParts(response) {
	if (!Array.isArray(response.blocks)) {
		return {
			reasoning: response.reasoning ?? '',
			content: response.content ?? '',
			toolCalls: response.tool_calls ?? []
		};
	}

	let reasoning = '';
	let content = '';
	const toolCalls = [];
	for (const block of response.blocks) {
		if ('reasoning' in block) reasoning += block.reasoning.text;
		else if ('text' in block) content += block.text;
		else if ('tool_call' in block) toolCalls.push(block.tool_call);
		else if ('Reasoning' in block) reasoning += block.Reasoning.text;
		else if ('Text' in block) content += block.Text;
		else if ('ToolCall' in block) toolCalls.push(block.ToolCall);
	}
	return { reasoning, content, toolCalls };
}

/**
 * Apply a tool-call outcome to its card, setting status plus result/error text.
 * @param {UiToolCall} call
 * @param {ToolOutcome | 'interrupted'} outcome
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
