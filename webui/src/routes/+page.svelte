<script>
	import { LoaderCircle, Plus, SquareTerminal } from '@lucide/svelte';
	import { archiveSession, createSession, listSessions } from '$lib/api.js';
	import SessionSidebar from '$lib/components/SessionSidebar.svelte';
	import SessionView from '$lib/components/SessionView.svelte';

	/** @type {import('$lib/api.js').SessionInfo[]} */
	let sessions = $state([]);
	/** @type {string | null} */
	let activeId = $state(null);
	let creating = $state(false);

	const active = $derived(sessions.find((s) => s.session_id === activeId) ?? null);

	async function refresh() {
		try {
			({ sessions } = await listSessions());
		} catch {
			// Server unreachable; keep the stale list. The per-session view shows
			// its own connection state.
		}
	}

	$effect(() => {
		refresh();
		const interval = setInterval(refresh, 10_000);
		return () => clearInterval(interval);
	});

	/** @param {string} model */
	async function handleCreate(model) {
		creating = true;
		try {
			const creds = await createSession(model);
			await refresh();
			activeId = creds.session_id;
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
		onSelect={(id) => (activeId = id)}
		onCreate={handleCreate}
		onArchive={handleArchive}
	/>

	<div class="flex min-w-0 flex-1 flex-col">
		<!-- Mobile header -->
		<div class="flex items-center gap-2 border-b border-border bg-sidebar px-4 py-2.5 md:hidden">
			<SquareTerminal class="size-4 text-primary" aria-hidden="true" />
			<span class="font-mono text-sm font-bold text-foreground">auger</span>
			{#if sessions.length > 0}
				<select
					value={activeId ?? ''}
					onchange={(e) => (activeId = e.currentTarget.value || null)}
					aria-label="Select session"
					class="ml-2 min-w-0 flex-1 rounded-md border border-border bg-card px-2 py-1 font-mono text-xs text-foreground"
				>
					<option value="">select session…</option>
					{#each sessions as s (s.session_id)}
						<option value={s.session_id}>{s.session_id.slice(0, 8)} · {s.model}{s.archived ? ' [archived]' : ''}</option>
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
				<SessionView session={active} />
			{/key}
		{:else}
			<!-- Welcome screen -->
			<div class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-6 auger-scroll">
				<div class="flex w-full max-w-md flex-col items-center gap-6 text-center">
					<pre
						aria-hidden="true"
						class="font-mono text-[10px] leading-tight text-primary select-none sm:text-xs">   ____ _  __  __ ____ _ ___  _____
  / __ '/ / / / / __ '/ _ \/ ___/
 / /_/ / /_/ / / /_/ /  __/ /
 \__,_/\__,_/  \__, /\___/_/
              /____/</pre>
					<div class="flex flex-col gap-2">
						<h2 class="font-mono text-lg font-bold text-foreground">agentic coding webui</h2>
						<p class="text-sm leading-relaxed text-pretty text-muted-foreground">
							A web client for the auger minimal coding agent harness. Streamed responses, tool
							calls with diffs, and human-in-the-loop approvals for shell, edit, and write.
						</p>
					</div>
					<button
						type="button"
						disabled={creating}
						onclick={() => handleCreate('')}
						class="flex items-center gap-2 rounded-md bg-primary px-5 py-2.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
					>
						{#if creating}
							<LoaderCircle class="size-4 animate-spin" aria-hidden="true" />
						{:else}
							<SquareTerminal class="size-4" aria-hidden="true" />
						{/if}
						Start a session
					</button>
					<div class="grid w-full grid-cols-1 gap-2 text-left sm:grid-cols-3">
						{#each [['sessions', 'create, resume, and delete sessions'], ['streaming', 'SSE deltas for reasoning + content'], ['approvals', 'gate shell, edit, and write calls']] as [title, desc] (title)}
							<div class="flex flex-col gap-1 rounded-lg border border-border bg-card p-3">
								<span class="font-mono text-xs font-semibold text-primary">{title}</span>
								<span class="text-[11px] leading-snug text-muted-foreground">{desc}</span>
							</div>
						{/each}
					</div>
				</div>
			</div>
		{/if}
	</div>
</main>
