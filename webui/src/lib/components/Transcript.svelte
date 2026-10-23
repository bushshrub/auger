<script>
	import { ArrowDown } from '@lucide/svelte';
	import { buildRows } from '$lib/transcript.js';
	import AssistantMessage from './AssistantMessage.svelte';
	import ToolCallCard from './ToolCallCard.svelte';
	import ToolCallGroup from './ToolCallGroup.svelte';
	import UserMessage from './UserMessage.svelte';

	/**
	 * @type {{
	 *   items: import('$lib/session.svelte.js').UiItem[],
	 *   sessionArchived?: boolean,
	 *   onRespond: (toolCallId: string, approved: boolean, message?: string) => Promise<void>
	 * }}
	 */
	let { items, sessionArchived = false, onRespond } = $props();

	const rows = $derived(buildRows(items));

	/** @type {HTMLDivElement | undefined} */
	let scrollEl = $state();
	let pinned = true;
	let showJump = $state(false);

	function onscroll() {
		if (!scrollEl) return;
		pinned = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 80;
		showJump = !pinned;
	}

	// Keep the transcript pinned to the bottom while content streams in, unless
	// the user has scrolled up. Reading the streamed fields registers them as
	// dependencies so the effect re-runs on every delta.
	$effect(() => {
		for (const item of items) {
			if (item.kind === 'assistant') {
				void item.content;
				void item.reasoning;
				void item.streaming;
			} else if (item.kind === 'tool') {
				void item.call.status;
				void item.call.result;
				void item.call.error;
			}
		}
		if (scrollEl && pinned) scrollEl.scrollTop = scrollEl.scrollHeight;
	});

	function jumpToBottom() {
		if (!scrollEl) return;
		pinned = true;
		showJump = false;
		scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: 'smooth' });
	}
</script>

<div class="relative flex min-h-0 flex-1 flex-col">
	<div bind:this={scrollEl} {onscroll} class="min-h-0 flex-1 overflow-y-auto auger-scroll">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 pt-8 pb-6">
			{#if items.length === 0}
				<p class="text-sm leading-relaxed text-muted-foreground">
					Ask the agent to explore the repo, fix a bug, or run a command. Reads run automatically;
					shell, edit and write wait for your approval.
				</p>
			{/if}

			{#each rows as row (row.id)}
				{#if row.kind === 'group'}
					<ToolCallGroup entries={row.entries} {sessionArchived} {onRespond} />
				{:else if row.item.kind === 'user'}
					<UserMessage text={row.item.text} />
				{:else if row.item.kind === 'assistant'}
					<AssistantMessage item={row.item} />
				{:else}
					<div class="rounded-lg border border-border">
						<ToolCallCard call={row.item.call} {sessionArchived} {onRespond} />
					</div>
				{/if}
			{/each}
		</div>
	</div>

	{#if showJump}
		<button
			type="button"
			onclick={jumpToBottom}
			aria-label="Jump to bottom"
			class="absolute bottom-3 left-1/2 flex size-8 -translate-x-1/2 items-center justify-center rounded-full border border-border bg-card shadow-md hover:bg-accent"
		>
			<ArrowDown class="size-4" aria-hidden="true" />
		</button>
	{/if}
</div>
