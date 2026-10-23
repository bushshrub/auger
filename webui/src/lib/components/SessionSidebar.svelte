<script>
	import { PanelLeft, Plus, Search } from '@lucide/svelte';
	import { sessionMeta, sessionTitle } from '$lib/session-meta.svelte.js';
	import SidebarRow from './SidebarRow.svelte';

	const LIMIT = 20;

	/**
	 * @type {{
	 *   sessions: import('$lib/api.js').SessionInfo[],
	 *   activeId: string | null,
	 *   creating: boolean,
	 *   open: boolean,
	 *   statuses: Record<string, import('$lib/status.svelte.js').SessionStatus>,
	 *   onSelect: (id: string) => void,
	 *   onCreate: () => void
	 * }}
	 */
	let { sessions, activeId, creating, open = $bindable(true), statuses, onSelect, onCreate } = $props();

	let searching = $state(false);
	let query = $state('');
	let showAll = $state(false);

	const filtered = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return q ? sessions.filter((s) => sessionTitle(s.session_id).toLowerCase().includes(q)) : sessions;
	});
	const searchActive = $derived(query.trim().length > 0);
	const visible = $derived(showAll || searchActive ? filtered : filtered.slice(0, LIMIT));

	/** @param {import('$lib/api.js').SessionInfo} s */
	function statusOf(s) {
		if (s.archived) return 'archived';
		const meta = sessionMeta[s.session_id];
		if (meta?.approval) return 'approval';
		if (meta?.busy || statuses[s.session_id] === 'working') return 'running';
		return 'idle';
	}
</script>

<aside
	class={`flex min-h-0 shrink-0 flex-col bg-sidebar text-sidebar-foreground max-md:hidden ${open ? 'w-64 border-r border-border' : 'w-0 overflow-hidden'}`}
	aria-label="Sessions"
>
	<div class="flex h-12 shrink-0 items-center justify-between px-3">
		<span class="px-1 text-sm font-semibold tracking-tight">auger</span>
		<button
			type="button"
			onclick={() => (open = false)}
			aria-label="Collapse sidebar"
			class="rounded-md p-1.5 text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
		>
			<PanelLeft class="size-4" aria-hidden="true" />
		</button>
	</div>

	<div class="flex flex-col gap-0.5 px-2">
		<button
			type="button"
			onclick={onCreate}
			disabled={creating}
			class="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm hover:bg-sidebar-accent disabled:opacity-50"
		>
			<Plus class="size-4" aria-hidden="true" />
			New
		</button>
		<button
			type="button"
			onclick={() => (searching = !searching)}
			aria-pressed={searching}
			class="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm hover:bg-sidebar-accent"
		>
			<Search class="size-4" aria-hidden="true" />
			Search
		</button>
		{#if searching}
			<input
				bind:value={query}
				placeholder="Filter sessions"
				aria-label="Filter sessions by title"
				class="mx-0.5 mt-1 rounded-md border border-border bg-background px-2 py-1 text-xs outline-none placeholder:text-muted-foreground focus:border-foreground/30"
			/>
		{/if}
	</div>

	<p class="px-4 pt-4 pb-1 text-xs font-medium text-muted-foreground">Recents</p>
	<nav class="min-h-0 flex-1 overflow-y-auto px-2 pb-3 auger-scroll">
		{#if sessions.length === 0}
			<p class="px-2.5 py-2 text-xs text-muted-foreground">No sessions yet</p>
		{:else if visible.length === 0}
			<p class="px-2.5 py-2 text-xs text-muted-foreground">No matches</p>
		{:else}
			<ul class="flex flex-col gap-0.5">
				{#each visible as s (s.session_id)}
					<SidebarRow
						title={sessionTitle(s.session_id)}
						active={s.session_id === activeId}
						status={statusOf(s)}
						onSelect={() => onSelect(s.session_id)}
					/>
				{/each}
			</ul>
		{/if}
		{#if !searchActive && filtered.length > LIMIT}
			<button
				type="button"
				onclick={() => (showAll = !showAll)}
				class="mt-1 px-2.5 py-1.5 text-xs text-muted-foreground hover:text-foreground"
			>
				{showAll ? 'Show less' : 'Show more'}
			</button>
		{/if}
	</nav>
</aside>
