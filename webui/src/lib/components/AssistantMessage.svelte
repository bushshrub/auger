<script>
	import Markdown from './Markdown.svelte';
	import ReasoningToggle from './ReasoningToggle.svelte';

	/** @type {{ item: Extract<import('$lib/session.svelte.js').UiItem, { kind: 'assistant' }> }} */
	let { item } = $props();

	const hasReasoning = $derived(item.reasoning.trim().length > 0);
	const hasContent = $derived(item.content.trim().length > 0);
	// The turn completed but the model streamed nothing renderable (e.g. a flaky
	// model tier returning zero deltas). Show a hint so it isn't silently blank.
	const noOutput = $derived(!!item.empty && !item.streaming && !hasReasoning && !hasContent);
</script>

<div class="flex flex-col gap-3">
	{#if hasReasoning}
		<ReasoningToggle text={item.reasoning} />
	{/if}
	{#if hasContent || item.streaming}
		<div class={item.streaming && hasContent ? 'auger-caret' : undefined}>
			{#if hasContent}
				<Markdown text={item.content} streaming={item.streaming} />
			{:else}
				<span class="text-sm text-muted-foreground auger-caret">Thinking</span>
			{/if}
		</div>
	{/if}
	{#if noOutput}
		<span class="text-sm italic text-muted-foreground">
			Model returned no output. The turn ended without a response.
		</span>
	{/if}
</div>
