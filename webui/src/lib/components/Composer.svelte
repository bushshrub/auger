<script>
	import { CornerDownLeft, LoaderCircle } from '@lucide/svelte';

	/**
	 * @type {{
	 *   busy: boolean,
	 *   disabled?: boolean,
	 *   onSend: (content: string) => Promise<void>
	 * }}
	 */
	let { busy, disabled = false, onSend } = $props();

	let value = $state('');
	let sending = $state(false);
	/** @type {HTMLTextAreaElement | undefined} */
	let textarea = $state();

	async function submit() {
		const content = value.trim();
		if (!content || busy || sending || disabled) return;
		sending = true;
		try {
			await onSend(content);
			value = '';
		} catch {
			// Failure is surfaced through the session error banner; keep the draft.
		} finally {
			sending = false;
			textarea?.focus();
		}
	}

	/** @param {KeyboardEvent} e */
	function onkeydown(e) {
		if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
			e.preventDefault();
			submit();
		}
	}
</script>

<div class="rounded-lg border border-border bg-card focus-within:border-primary/50">
	<div class="flex items-end gap-2 px-3 py-2">
		<span class="pb-2 font-mono text-sm font-bold text-primary select-none" aria-hidden="true">
			&gt;
		</span>
		<textarea
			bind:this={textarea}
			bind:value
			{onkeydown}
			rows="2"
			placeholder={busy ? 'Agent is working…' : 'Give the agent a coding task…'}
			{disabled}
			aria-label="Message the agent"
			class="max-h-48 min-h-12 flex-1 resize-none bg-transparent py-2 font-mono text-sm text-foreground outline-none placeholder:text-muted-foreground/60 disabled:opacity-50 auger-scroll"
		></textarea>
		<button
			type="button"
			onclick={submit}
			disabled={!value.trim() || busy || sending || disabled}
			class="mb-1 flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-40"
		>
			{#if busy || sending}
				<LoaderCircle class="size-3.5 animate-spin" aria-hidden="true" />
			{:else}
				<CornerDownLeft class="size-3.5" aria-hidden="true" />
			{/if}
			Send
		</button>
	</div>
	<div class="flex items-center justify-between border-t border-border px-4 py-1.5">
		<p class="font-mono text-[10px] text-muted-foreground">enter to send · shift+enter for newline</p>
		<p class="font-mono text-[10px] text-muted-foreground">shell / edit / write require approval</p>
	</div>
</div>
