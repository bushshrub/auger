// Turns the flat transcript (UiItem[]) into display rows. A run of tool calls
// in one assistant turn, with the reasoning-only turns between them, folds into
// one group row so a burst of commands reads as a single line.

/**
 * @typedef {import('./session.svelte.js').UiItem} UiItem
 * @typedef {import('./session.svelte.js').UiToolCall} UiToolCall
 * @typedef {{ kind: 'reasoning', id: string, text: string }
 *         | { kind: 'tool', id: string, call: UiToolCall }} GroupEntry
 * @typedef {{ kind: 'item', id: string, item: UiItem }
 *         | { kind: 'group', id: string, entries: GroupEntry[] }} Row
 */

/** @param {UiItem} it */
const isTool = (it) => it.kind === 'tool';

// A finished assistant turn that only carried reasoning. These sit between the
// tool calls of a burst, so they fold into the group to keep the step order.
/** @param {UiItem} it */
const isFoldableReasoning = (it) =>
	it.kind === 'assistant' &&
	!it.streaming &&
	it.content.trim().length === 0 &&
	it.reasoning.trim().length > 0;

/**
 * @param {UiItem[]} items
 * @returns {Row[]}
 */
export function buildRows(items) {
	/** @type {Row[]} */
	const out = [];
	let i = 0;
	while (i < items.length) {
		const item = items[i];
		if (!isTool(item) && !isFoldableReasoning(item)) {
			out.push({ kind: 'item', id: item.id, item });
			i++;
			continue;
		}
		/** @type {UiItem[]} */
		const run = [];
		while (i < items.length && (isTool(items[i]) || isFoldableReasoning(items[i]))) {
			run.push(items[i]);
			i++;
		}
		if (run.filter(isTool).length < 2) {
			for (const r of run) out.push({ kind: 'item', id: r.id, item: r });
			continue;
		}
		out.push({
			kind: 'group',
			id: `group:${run[0].id}`,
			entries: run.map((r) =>
				r.kind === 'tool'
					? { kind: 'tool', id: r.id, call: r.call }
					: { kind: 'reasoning', id: r.id, text: r.kind === 'assistant' ? r.reasoning : '' }
			)
		});
	}
	return out;
}
