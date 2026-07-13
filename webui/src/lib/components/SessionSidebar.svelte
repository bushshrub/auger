<script>
	import { Archive, LoaderCircle, Plus, SquareTerminal } from '@lucide/svelte';

	/**
	 * @type {{
	 *   sessions: import('$lib/api.js').SessionInfo[],
	 *   activeId: string | null,
	 *   creating: boolean,
	 *   onSelect: (id: string) => void,
	 *   onCreate: (model: string) => void,
	 *   onArchive: (id: string) => void
	 * }}
	 */
	let { sessions, activeId, creating, onSelect, onCreate, onArchive } = $props();

	const activeSessions = $derived(sessions.filter((s) => !s.archived));
	const archivedSessions = $derived(sessions.filter((s) => s.archived));

	// Empty means "let the server pick its default model".
	let model = $state('');

	/** @param {number} createdAt unix seconds */
	function formatTime(createdAt) {
		return new Date(createdAt * 1000).toLocaleTimeString([], {
			hour: '2-digit',
			minute: '2-digit'
		});
	}
</script>

<aside class="flex w-64 shrink-0 flex-col border-r border-border bg-sidebar max-md:hidden">
	<div class="flex items-center gap-2 border-b border-border px-4 py-3">
		<SquareTerminal class="size-4 text-primary" aria-hidden="true" />
		<h1 class="font-mono text-sm font-bold tracking-tight text-foreground">auger</h1>
		<span class="ml-auto rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
			webui
		</span>
	</div>

	<div class="flex flex-col gap-2 border-b border-border p-3">
		<label
			for="model-input"
			class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
		>
			model
		</label>
		<input
			id="model-input"
			bind:value={model}
			placeholder="server default"
			class="rounded-md border border-border bg-card px-2 py-1.5 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:border-primary/50"
		/>
		<button
			type="button"
			disabled={creating}
			onclick={() => onCreate(model.trim())}
			class="flex items-center justify-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
		>
			{#if creating}
				<LoaderCircle class="size-3.5 animate-spin" aria-hidden="true" />
			{:else}
				<Plus class="size-3.5" aria-hidden="true" />
			{/if}
			New session
		</button>
	</div>

	<nav aria-label="Sessions" class="min-h-0 flex-1 overflow-y-auto p-2 auger-scroll">
		{#if sessions.length === 0}
			<p class="px-2 py-3 font-mono text-xs text-muted-foreground">no sessions yet</p>
		{:else}
			{#if activeSessions.length > 0}
				<ul class="flex flex-col gap-1">
					{#each activeSessions as s (s.session_id)}
						<li class="group relative">
							<button
								type="button"
								onclick={() => onSelect(s.session_id)}
								aria-current={s.session_id === activeId ? 'true' : undefined}
								class={`flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left hover:bg-sidebar-accent ${s.session_id === activeId ? 'bg-sidebar-accent' : ''}`}
							>
								<span
									class={`font-mono text-xs ${s.session_id === activeId ? 'text-primary' : 'text-foreground'}`}
								>
									{s.session_id.slice(0, 8)}
								</span>
								<span class="font-mono text-[10px] text-muted-foreground">
									{s.model} · {formatTime(s.created_at)}
								</span>
							</button>
							<button
								type="button"
								onclick={() => onArchive(s.session_id)}
								aria-label={`Archive session ${s.session_id.slice(0, 8)}`}
								class="absolute top-1/2 right-2 -translate-y-1/2 rounded p-1 text-muted-foreground opacity-0 group-hover:opacity-100 hover:bg-muted hover:text-foreground focus-visible:opacity-100"
							>
								<Archive class="size-3.5" aria-hidden="true" />
							</button>
						</li>
					{/each}
				</ul>
			{/if}

			{#if archivedSessions.length > 0}
				<p class="px-2 pt-3 pb-1 font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
					archived
				</p>
				<ul class="flex flex-col gap-1">
					{#each archivedSessions as s (s.session_id)}
						<li>
							<button
								type="button"
								onclick={() => onSelect(s.session_id)}
								aria-current={s.session_id === activeId ? 'true' : undefined}
								class={`flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left opacity-60 hover:bg-sidebar-accent hover:opacity-100 ${s.session_id === activeId ? 'bg-sidebar-accent opacity-100' : ''}`}
							>
								<span class="font-mono text-xs text-muted-foreground">
									{s.session_id.slice(0, 8)}
								</span>
								<span class="font-mono text-[10px] text-muted-foreground">
									{s.model} · {formatTime(s.created_at)}
								</span>
							</button>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</nav>

	<footer class="border-t border-border px-4 py-2.5">
		<a
			href="https://github.com/bushshrub/auger"
			target="_blank"
			rel="noreferrer"
			class="font-mono text-[10px] text-muted-foreground hover:text-foreground"
		>
			bushshrub/auger · AGPL-3.0
		</a>
	</footer>
</aside>
