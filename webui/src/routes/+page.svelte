<script>
	import { LoaderCircle, PanelLeft, Plus } from '@lucide/svelte';
	import { archiveSession, createSession, listSessions } from '$lib/api.js';
	import { sessionTitle } from '$lib/session-meta.svelte.js';
	import { page } from '$app/state';
	import { StatusTracker } from '$lib/status.svelte.js';
	import SessionSidebar from '$lib/components/SessionSidebar.svelte';
	import SessionView from '$lib/components/SessionView.svelte';

	/** @type {import('$lib/api.js').SessionInfo[]} */
	let sessions = $state([]);
	/** @type {string | null} */
	// ?session=<id> opens a session directly.
	let activeId = $state(page.url.searchParams.get('session'));
	let creating = $state(false);
	let sidebarOpen = $state(true);

	const tracker = new StatusTracker();

	const active = $derived(sessions.find((s) => s.session_id === activeId) ?? null);

	async function refresh() {
		try {
			({ sessions } = await listSessions());
			tracker.sync(sessions);
		} catch {
			// Server unreachable; keep the stale list. The per-session view shows
			// its own connection state.
		}
	}

	$effect(() => {
		refresh();
		const interval = setInterval(refresh, 10_000);
		return () => {
			clearInterval(interval);
			tracker.stop();
		};
	});

	/** @param {string} model */
	async function handleCreate(model = '') {
		creating = true;
		try {
			const creds = await createSession(model);
			// Insert the new session optimistically and switch to it immediately, so
			// the view opens without waiting on a second /sessions round-trip.
			// refresh() reconciles created_at/archived shortly after.
			/** @type {import('$lib/api.js').SessionInfo} */
			const info = {
				session_id: creds.session_id,
				model: creds.model,
				created_at: new Date().toISOString(),
				context_window: creds.context_window,
				tokens: creds.tokens,
				archived: false
			};
			sessions = [info, ...sessions.filter((s) => s.session_id !== info.session_id)];
			activeId = creds.session_id;
			refresh();
		} finally {
			creating = false;
		}
	}

	/** @param {string} id */
	async function handleArchive(id) {
		const session = sessions.find((s) => s.session_id === id);
		if (!session) return;
		try {
			await archiveSession(id, session.tokens.write);
		} catch {
			// Non-2xx: refresh anyway to reconcile local state with server.
		}
		// Do not clear activeId -- archived sessions remain selectable.
		await refresh();
	}
</script>

<main class="flex h-dvh bg-background">
	<SessionSidebar
		{sessions}
		{activeId}
		{creating}
		bind:open={sidebarOpen}
		statuses={tracker.statuses}
		onSelect={(id) => (activeId = id)}
		onCreate={() => handleCreate('')}
	/>

	<div class="flex min-w-0 flex-1 flex-col">
		<!-- Mobile header -->
		<div class="flex items-center gap-2 border-b border-border bg-sidebar px-4 py-2.5 md:hidden">
			<span class="text-sm font-semibold">auger</span>
			{#if sessions.length > 0}
				<select
					value={activeId ?? ''}
					onchange={(e) => (activeId = e.currentTarget.value || null)}
					aria-label="Select session"
					class="ml-2 min-w-0 flex-1 rounded-md border border-border bg-card px-2 py-1 text-xs text-foreground"
				>
					<option value="">Select a session</option>
					{#each sessions as s (s.session_id)}
						<option value={s.session_id}>{sessionTitle(s.session_id)}{s.archived ? ' (archived)' : ''}</option>
					{/each}
				</select>
			{/if}
			<button
				type="button"
				disabled={creating}
				onclick={() => handleCreate('')}
				aria-label="New session"
				class="ml-auto flex items-center gap-1 rounded-md bg-primary px-2.5 py-1.5 text-xs font-medium text-primary-foreground disabled:opacity-50"
			>
				{#if creating}
					<LoaderCircle class="size-3.5 animate-spin" aria-hidden="true" />
				{:else}
					<Plus class="size-3.5" aria-hidden="true" />
				{/if}
				New
			</button>
		</div>

		{#if active}
			{#key active.session_id}
				<SessionView
					session={active}
					{sidebarOpen}
					onToggleSidebar={() => (sidebarOpen = true)}
					onArchive={handleArchive}
				/>
			{/key}
		{:else}
			<div class="flex h-12 shrink-0 items-center px-3">
				{#if !sidebarOpen}
					<button
						type="button"
						onclick={() => (sidebarOpen = true)}
						aria-label="Show sidebar"
						class="rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-foreground"
					>
						<PanelLeft class="size-4" aria-hidden="true" />
					</button>
				{/if}
			</div>
			<div class="flex min-h-0 flex-1 flex-col items-center justify-center gap-5 p-6 text-center">
				<h2 class="text-2xl font-semibold tracking-tight">auger</h2>
				<p class="max-w-sm text-sm leading-relaxed text-pretty text-muted-foreground">
					A web client for the auger agent harness. Streamed replies, tool calls, and approvals
					for shell, edit, and write.
				</p>
				<button
					type="button"
					disabled={creating}
					onclick={() => handleCreate('')}
					class="flex items-center gap-2 rounded-full bg-primary px-5 py-2 text-sm font-medium text-primary-foreground hover:opacity-90 disabled:opacity-50"
				>
					{#if creating}
						<LoaderCircle class="size-4 animate-spin" aria-hidden="true" />
					{:else}
						<Plus class="size-4" aria-hidden="true" />
					{/if}
					Start a session
				</button>
			</div>
		{/if}
	</div>
</main>
