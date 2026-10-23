<script>
	import { ChevronDown, ChevronRight } from '@lucide/svelte';

	/** @type {{ text: string }} */
	let { text } = $props();

	let show = $state(false);
</script>

<div>
	<button
		type="button"
		onclick={() => (show = !show)}
		class="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
		aria-expanded={show}
	>
		{#if show}
			<ChevronDown class="size-3.5" aria-hidden="true" />
		{:else}
			<ChevronRight class="size-3.5" aria-hidden="true" />
		{/if}
		<span>Thought</span>
	</button>
	{#if show}
		<p
			class="mt-2 border-l border-border pl-3 text-sm leading-relaxed whitespace-pre-wrap text-muted-foreground"
		>
			{text}
		</p>
	{/if}
</div>
