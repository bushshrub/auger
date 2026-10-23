<script>
	import { Archive, Info, PanelLeft } from '@lucide/svelte';
	import { renameSession } from '$lib/session-meta.svelte.js';

	/**
	 * @type {{
	 *   id: string,
	 *   title: string,
	 *   model: string,
	 *   archived: boolean,
	 *   connected: boolean,
	 *   sidebarOpen: boolean,
	 *   infoOpen: boolean,
	 *   onToggleSidebar: () => void,
	 *   onToggleInfo: () => void,
	 *   onArchive: () => void
	 * }}
	 */
	let {
		id,
		title,
		model,
		archived,
		connected,
		sidebarOpen,
		infoOpen,
		onToggleSidebar,
		onToggleInfo,
		onArchive
	} = $props();

	let editing = $state(false);
	let draft = $state('');

	/** @param {HTMLInputElement} node */
	function focusOnMount(node) {
		node.focus();
	}

	function startEdit() {
		draft = title;
		editing = true;
	}

	// Enter and blur both commit; the flag makes the second one a no-op. Escape
	// clears the flag first, so the blur from unmounting the input does nothing.
	function commit() {
		if (!editing) return;
		editing = false;
		renameSession(id, draft);
	}

	/** @param {KeyboardEvent} e */
	function onkeydown(e) {
		if (e.key === 'Enter') commit();
		else if (e.key === 'Escape') editing = false;
	}
</script>

<header class="flex h-12 shrink-0 items-center gap-2 border-b border-border px-3">
	{#if !sidebarOpen}
		<button
			type="button"
			onclick={onToggleSidebar}
			aria-label="Show sidebar"
			class="rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-foreground"
		>
			<PanelLeft class="size-4" aria-hidden="true" />
		</button>
	{/if}

	{#if editing}
		<input
			bind:value={draft}
			{onkeydown}
			onblur={commit}
			aria-label="Session title"
			use:focusOnMount
			class="min-w-0 max-w-md rounded-md border border-border bg-card px-2 py-1 text-sm outline-none focus:border-foreground/30"
		/>
	{:else}
		<button
			type="button"
			onclick={startEdit}
			title="Rename"
			class="min-w-0 truncate rounded-md px-2 py-1 text-sm font-medium hover:bg-accent"
		>
			{title}
		</button>
	{/if}

	{#if !connected && !archived}
		<span class="text-xs text-destructive">reconnecting</span>
	{/if}
	{#if archived}
		<span class="rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground">archived</span>
	{/if}

	<span class="hidden min-w-0 truncate text-xs text-muted-foreground md:inline">{model}</span>

	<div class="ml-auto flex items-center gap-0.5">
		{#if !archived}
			<button
				type="button"
				onclick={onArchive}
				aria-label="Archive session"
				title="Archive"
				class="rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-foreground"
			>
				<Archive class="size-4" aria-hidden="true" />
			</button>
		{/if}
		<button
			type="button"
			onclick={onToggleInfo}
			aria-label="Session info"
			aria-pressed={infoOpen}
			class={`rounded-md p-1.5 hover:bg-accent hover:text-foreground ${infoOpen ? 'bg-accent text-foreground' : 'text-muted-foreground'}`}
		>
			<Info class="size-4" aria-hidden="true" />
		</button>
	</div>
</header>
