<script>
	import { X } from '@lucide/svelte';

	/** @type {{ message: string, tone?: 'error' | 'info', ondismiss: () => void }} */
	let { message, tone = 'info', ondismiss } = $props();
</script>

<div
	role="status"
	class={`flex items-center gap-2 rounded-lg border bg-card px-3 py-1.5 text-xs ${tone === 'error' ? 'border-destructive/40 text-destructive' : 'border-border text-muted-foreground'}`}
>
	<span class="min-w-0 flex-1 break-words">{message}</span>
	<button
		type="button"
		onclick={ondismiss}
		aria-label="Dismiss notice"
		class="shrink-0 rounded p-0.5 hover:bg-accent hover:text-foreground"
	>
		<X class="size-3.5" aria-hidden="true" />
	</button>
</div>
