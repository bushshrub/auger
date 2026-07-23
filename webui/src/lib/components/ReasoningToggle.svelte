<script>
	import { Brain, ChevronDown, ChevronRight } from '@lucide/svelte';

	/** @type {{ text: string }} */
	let { text } = $props();

	let show = $state(false);
</script>

<div>
	<button
		type="button"
		onclick={() => (show = !show)}
		class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
		aria-expanded={show}
	>
		{#if show}
			<ChevronDown class="size-3" aria-hidden="true" />
		{:else}
			<ChevronRight class="size-3" aria-hidden="true" />
		{/if}
		<Brain class="size-3" aria-hidden="true" />
		<span class="font-mono">reasoning</span>
	</button>
	{#if show}
		<p class="mt-1.5 border-l-2 border-border pl-3 text-xs italic leading-relaxed text-muted-foreground">
			{text}
		</p>
	{/if}
</div>
