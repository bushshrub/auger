<script>
	import { Check, ChevronDown, ChevronRight, LoaderCircle, X } from '@lucide/svelte';
	import { toolLabel } from '$lib/tool-labels.js';
	import ToolCallDetail from './ToolCallDetail.svelte';

	/**
	 * One tool call as a single line. Pending calls show approve/deny inline;
	 * pending and failed calls open by themselves.
	 * @type {{
	 *   call: import('$lib/session.svelte.js').UiToolCall,
	 *   sessionArchived?: boolean,
	 *   onRespond: (toolCallId: string, approved: boolean, message?: string) => Promise<void>
	 * }}
	 */
	let { call, sessionArchived = false, onRespond } = $props();

	let open = $state(false);
	let responding = $state(false);

	const label = $derived(toolLabel(call.name, call.arguments));
	const verb = $derived(label.split(' ')[0]);
	const rest = $derived(label.slice(verb.length).trim());
	const isPending = $derived(call.status === 'pending_approval');
	const isError = $derived(call.status === 'error');

	$effect(() => {
		if (isPending || isError) open = true;
	});

	/** @param {boolean} approved */
	async function respond(approved) {
		responding = true;
		try {
			await onRespond(call.id, approved);
		} catch {
			// Failure is surfaced through the session error banner.
		} finally {
			responding = false;
		}
	}
</script>

<div>
	<div class="flex items-center gap-2 px-3 py-2 text-sm">
		<button
			type="button"
			onclick={() => (open = !open)}
			aria-expanded={open}
			class="flex min-w-0 flex-1 items-center gap-1.5 text-left"
		>
			{#if open}
				<ChevronDown class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
			{:else}
				<ChevronRight class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
			{/if}
			<span class="shrink-0 text-muted-foreground">{verb}</span>
			<span class={`min-w-0 truncate ${isError ? 'text-destructive' : 'text-foreground'}`}>{rest}</span>
		</button>

		{#if isPending && !sessionArchived}
			<div class="flex shrink-0 items-center gap-1.5">
				<button
					type="button"
					disabled={responding}
					onclick={() => respond(false)}
					class="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-accent disabled:opacity-50"
				>
					<X class="size-3" aria-hidden="true" />
					Deny
				</button>
				<button
					type="button"
					disabled={responding}
					onclick={() => respond(true)}
					class="flex items-center gap-1 rounded-md bg-primary px-2 py-1 text-xs font-medium text-primary-foreground hover:opacity-90 disabled:opacity-50"
				>
					<Check class="size-3" aria-hidden="true" />
					Approve
				</button>
			</div>
		{:else if isPending}
			<span class="shrink-0 text-xs text-warning">waiting for approval</span>
		{:else if call.status === 'running'}
			<LoaderCircle class="size-3.5 shrink-0 animate-spin text-muted-foreground" aria-label="Running" />
		{:else if isError}
			<span class="shrink-0 text-xs text-destructive">failed</span>
		{:else if call.status === 'denied'}
			<span class="shrink-0 text-xs text-muted-foreground">denied</span>
		{/if}
	</div>

	{#if open}
		<ToolCallDetail {call} />
	{/if}
</div>
