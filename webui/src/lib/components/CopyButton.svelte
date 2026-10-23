<script>
	import { Check, Copy } from '@lucide/svelte';

	/** @type {{ text: string }} */
	let { text } = $props();

	let copied = $state(false);

	function copy() {
		navigator.clipboard?.writeText(text).then(
			() => {
				copied = true;
				setTimeout(() => (copied = false), 1500);
			},
			() => {}
		);
	}
</script>

<button
	type="button"
	onclick={copy}
	aria-label={copied ? 'Copied' : 'Copy'}
	class="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
>
	{#if copied}
		<Check class="size-3" aria-hidden="true" />
		Copied
	{:else}
		<Copy class="size-3" aria-hidden="true" />
		Copy
	{/if}
</button>
