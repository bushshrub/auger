<script>
	import {
		Check,
		ChevronDown,
		ChevronRight,
		CircleAlert,
		Layers,
		LoaderCircle
	} from '@lucide/svelte';
	import ReasoningToggle from './ReasoningToggle.svelte';
	import ToolCallCard from './ToolCallCard.svelte';

	/**
	 * `entries` is the ordered timeline of an exploration burst: reasoning notes
	 * interleaved with their read-only tool calls.
	 * @type {{
	 *   entries: ({ kind: 'reasoning', id: string, text: string }
	 *           | { kind: 'tool', id: string, call: import('$lib/session.svelte.js').UiToolCall })[],
	 *   sessionArchived?: boolean,
	 *   onRespond: (toolCallId: string, approved: boolean, message?: string) => Promise<void>
	 * }}
	 */
	let { entries, sessionArchived = false, onRespond } = $props();

	// The tool calls drive the collapsed summary and status; reasoning entries
	// only surface once the group is expanded, preserving the step order.
	const calls = $derived(
		entries.flatMap((e) => (e.kind === 'tool' ? [e.call] : []))
	);

	// How each groupable tool is phrased in the collapsed summary. Rendered in
	// this fixed order so the summary reads consistently regardless of call order.
	/** @type {[string, string, (n: number) => string][]} */
	const SUMMARY = [
		['read_file', 'read', (n) => `${n} file${n === 1 ? '' : 's'}`],
		['list_files', 'listed', (n) => `${n} director${n === 1 ? 'y' : 'ies'}`],
		['grep', 'searched', (n) => `${n} pattern${n === 1 ? '' : 's'}`],
		['glob', 'globbed', (n) => `${n} pattern${n === 1 ? '' : 's'}`],
		['todo_list', 'updated', () => 'todos']
	];

	const summary = $derived.by(() => {
		/** @type {Record<string, number>} */
		const counts = {};
		for (const call of calls) counts[call.name] = (counts[call.name] ?? 0) + 1;
		return SUMMARY.filter(([name]) => counts[name])
			.map(([name, verb, noun]) => `${verb} ${noun(counts[name])}`)
			.join(', ');
	});

	const anyPending = $derived(calls.some((c) => c.status === 'pending_approval'));
	const anyRunning = $derived(calls.some((c) => c.status === 'running'));
	const anyError = $derived(calls.some((c) => c.status === 'error' || c.status === 'denied'));

	// Auto-open when a call needs attention so it is never hidden behind the summary.
	let open = $state(false);
	$effect(() => {
		if (anyPending) open = true;
	});
</script>

<div class="overflow-hidden rounded-lg border border-border bg-card">
	<button
		type="button"
		onclick={() => (open = !open)}
		class="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-accent/40"
		aria-expanded={open}
	>
		{#if open}
			<ChevronDown class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
		{:else}
			<ChevronRight class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
		{/if}
		<Layers class="size-3.5 shrink-0 text-primary" aria-hidden="true" />
		<span class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{summary}</span>

		{#if anyPending || anyRunning}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-muted px-2 py-0.5 font-mono text-[10px] text-muted-foreground"
			>
				<LoaderCircle class="size-3 animate-spin" aria-hidden="true" />
				{anyPending ? 'needs approval' : 'running'}
			</span>
		{:else if anyError}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 font-mono text-[10px] text-destructive"
			>
				<CircleAlert class="size-3" aria-hidden="true" />
				{calls.length}
			</span>
		{:else}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-success/15 px-2 py-0.5 font-mono text-[10px] text-success"
			>
				<Check class="size-3" aria-hidden="true" />
				{calls.length}
			</span>
		{/if}
	</button>

	{#if open}
		<div class="flex flex-col gap-2 border-t border-border p-2">
			{#each entries as entry (entry.id)}
				{#if entry.kind === 'reasoning'}
					<ReasoningToggle text={entry.text} />
				{:else}
					<ToolCallCard call={entry.call} {sessionArchived} {onRespond} />
				{/if}
			{/each}
		</div>
	{/if}
</div>
