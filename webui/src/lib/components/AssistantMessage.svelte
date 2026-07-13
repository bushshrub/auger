<script>
	import { Brain, ChevronDown, ChevronRight } from '@lucide/svelte';
	import Markdown from './Markdown.svelte';

	/** @type {{ item: Extract<import('$lib/session.svelte.js').UiItem, { kind: 'assistant' }> }} */
	let { item } = $props();

	let showReasoning = $state(false);
	const hasReasoning = $derived(item.reasoning.trim().length > 0);
	const hasContent = $derived(item.content.trim().length > 0);
</script>

<div class="flex flex-col gap-2 pl-7">
	{#if hasReasoning}
		<div>
			<button
				type="button"
				onclick={() => (showReasoning = !showReasoning)}
				class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
				aria-expanded={showReasoning}
			>
				{#if showReasoning}
					<ChevronDown class="size-3" aria-hidden="true" />
				{:else}
					<ChevronRight class="size-3" aria-hidden="true" />
				{/if}
				<Brain class="size-3" aria-hidden="true" />
				<span class="font-mono">reasoning</span>
			</button>
			{#if showReasoning}
				<p class="mt-1.5 border-l-2 border-border pl-3 text-xs italic leading-relaxed text-muted-foreground">
					{item.reasoning}
				</p>
			{/if}
		</div>
	{/if}
	{#if hasContent || item.streaming}
		<div class={item.streaming && hasContent ? 'auger-caret' : undefined}>
			{#if hasContent}
				<Markdown text={item.content} />
			{:else}
				<span class="font-mono text-xs text-muted-foreground auger-caret">thinking</span>
			{/if}
		</div>
	{/if}
</div>
