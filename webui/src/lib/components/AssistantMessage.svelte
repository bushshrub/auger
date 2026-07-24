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

<div class="flex flex-col gap-2 pl-7">
	{#if hasReasoning}
		<ReasoningToggle text={item.reasoning} />
	{/if}
	{#if hasContent || item.streaming}
		<div class={item.streaming && hasContent ? 'auger-caret' : undefined}>
			{#if hasContent}
				<Markdown text={item.content} streaming={item.streaming} />
			{:else}
				<span class="font-mono text-xs text-muted-foreground auger-caret">thinking</span>
			{/if}
		</div>
	{/if}
	{#if noOutput}
		<span class="font-mono text-xs italic text-muted-foreground">
			model returned no output — the turn ended without any response
		</span>
	{/if}
</div>
