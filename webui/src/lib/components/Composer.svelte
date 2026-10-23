<script>
	import { ArrowUp, Square } from '@lucide/svelte';

	/**
	 * @type {{
	 *   busy: boolean,
	 *   disabled?: boolean,
	 *   model?: string,
	 *   onSend: (content: string) => Promise<void>,
	 *   onInterrupt: () => void
	 * }}
	 */
	let { busy, disabled = false, model = '', onSend, onInterrupt } = $props();

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

<div class="flex flex-col gap-1.5">
	<div
		class="flex items-end gap-2 rounded-2xl border border-border bg-card px-3 py-2 shadow-sm focus-within:border-foreground/25"
	>
		<textarea
			bind:this={textarea}
			bind:value
			{onkeydown}
			rows="1"
			placeholder="Type / for commands"
			{disabled}
			aria-label="Message the agent"
			class="max-h-48 min-h-7 flex-1 resize-none bg-transparent py-1.5 text-[0.9375rem] outline-none placeholder:text-muted-foreground disabled:opacity-50 [field-sizing:content] auger-scroll"
		></textarea>
		{#if busy}
			<button
				type="button"
				onclick={onInterrupt}
				aria-label="Stop"
				title="Stop"
				class="mb-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-foreground text-background hover:opacity-80"
			>
				<Square class="size-3.5 fill-current" aria-hidden="true" />
			</button>
		{:else}
			<button
				type="button"
				onclick={submit}
				disabled={!value.trim() || sending || disabled}
				aria-label="Send"
				title="Send"
				class="mb-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-30"
			>
				<ArrowUp class="size-4" aria-hidden="true" />
			</button>
		{/if}
	</div>
	<p class="text-center text-xs text-muted-foreground">
		{model || 'server default model'}
		<span class="hidden sm:inline">| Enter to send, Shift+Enter for a new line</span>
	</p>
</div>
