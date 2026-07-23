<script>
	import Markdown from './Markdown.svelte';
	import ReasoningToggle from './ReasoningToggle.svelte';

	/** @type {{ item: Extract<import('$lib/session.svelte.js').UiItem, { kind: 'assistant' }> }} */
	let { item } = $props();

	const hasReasoning = $derived(item.reasoning.trim().length > 0);
	const hasContent = $derived(item.content.trim().length > 0);
</script>

<div class="flex flex-col gap-2 pl-7">
	{#if hasReasoning}
		<ReasoningToggle text={item.reasoning} />
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
