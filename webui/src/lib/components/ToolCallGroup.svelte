<script>
	import { ChevronDown, ChevronRight, CircleAlert, LoaderCircle } from '@lucide/svelte';
	import { groupLabel } from '$lib/tool-labels.js';
	import ReasoningToggle from './ReasoningToggle.svelte';
	import ToolCallCard from './ToolCallCard.svelte';

	/**
	 * A burst of tool calls in one assistant turn. Collapsed it is one summary
	 * line; expanded, a bordered list with one row per call and the reasoning
	 * notes between them.
	 * @type {{
	 *   entries: ({ kind: 'reasoning', id: string, text: string }
	 *           | { kind: 'tool', id: string, call: import('$lib/session.svelte.js').UiToolCall })[],
	 *   sessionArchived?: boolean,
	 *   onRespond: (toolCallId: string, approved: boolean, message?: string) => Promise<void>
	 * }}
	 */
	let { entries, sessionArchived = false, onRespond } = $props();

	const calls = $derived(entries.flatMap((e) => (e.kind === 'tool' ? [e.call] : [])));
	const label = $derived(groupLabel(calls));
	const anyPending = $derived(calls.some((c) => c.status === 'pending_approval'));
	const anyRunning = $derived(calls.some((c) => c.status === 'running'));
	const anyError = $derived(calls.some((c) => c.status === 'error'));

	let open = $state(false);
	$effect(() => {
		if (anyPending || anyError) open = true;
	});
</script>

<div class="flex flex-col">
	<button
		type="button"
		onclick={() => (open = !open)}
		aria-expanded={open}
		class="flex max-w-full items-center gap-1.5 py-1 text-left text-sm text-muted-foreground hover:text-foreground"
	>
		{#if open}
			<ChevronDown class="size-3.5 shrink-0" aria-hidden="true" />
		{:else}
			<ChevronRight class="size-3.5 shrink-0" aria-hidden="true" />
		{/if}
		<span class="truncate">{label}</span>
		{#if anyPending}
			<span class="shrink-0 text-xs text-warning">needs approval</span>
		{:else if anyRunning}
			<LoaderCircle class="size-3.5 shrink-0 animate-spin" aria-label="Running" />
		{:else if anyError}
			<CircleAlert class="size-3.5 shrink-0 text-destructive" aria-label="Failed" />
		{/if}
	</button>

	{#if open}
		<div class="mt-1 divide-y divide-border overflow-hidden rounded-lg border border-border">
			{#each entries as entry (entry.id)}
				{#if entry.kind === 'reasoning'}
					<div class="px-3 py-2">
						<ReasoningToggle text={entry.text} />
					</div>
				{:else}
					<ToolCallCard call={entry.call} {sessionArchived} {onRespond} />
				{/if}
			{/each}
		</div>
	{/if}
</div>
