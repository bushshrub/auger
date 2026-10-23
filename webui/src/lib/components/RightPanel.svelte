<script>
	/**
	 * @type {{
	 *   session: import('$lib/api.js').SessionInfo,
	 *   contextTokens: number,
	 *   connected: boolean
	 * }}
	 */
	let { session, contextTokens, connected } = $props();

	const contextPct = $derived(
		Math.min(100, Math.round((contextTokens / Math.max(1, session.context_window)) * 100))
	);
	const created = $derived(new Date(session.created_at).toLocaleString());
</script>

<aside class="flex w-72 shrink-0 flex-col gap-5 overflow-y-auto border-l border-border bg-background p-4 text-sm max-md:hidden auger-scroll">
	<h2 class="text-sm font-semibold">Session</h2>
	<dl class="flex flex-col gap-3 text-xs">
		<div>
			<dt class="text-muted-foreground">Model</dt>
			<dd class="mt-0.5 font-mono break-all">{session.model}</dd>
		</div>
		<div>
			<dt class="text-muted-foreground">Created</dt>
			<dd class="mt-0.5">{created}</dd>
		</div>
		<div>
			<dt class="text-muted-foreground">Session id</dt>
			<dd class="mt-0.5 font-mono break-all">{session.session_id}</dd>
		</div>
		<div>
			<dt class="text-muted-foreground">Status</dt>
			<dd class="mt-0.5">
				{session.archived ? 'Archived' : connected ? 'Connected' : 'Reconnecting'}
			</dd>
		</div>
	</dl>
	<div class="flex flex-col gap-1.5">
		<div class="flex justify-between text-xs">
			<span class="text-muted-foreground">Context</span>
			<span class="font-mono">{contextPct}%</span>
		</div>
		<div class="h-1.5 overflow-hidden rounded-full bg-muted">
			<div
				class={`h-full rounded-full ${contextPct > 80 ? 'bg-destructive' : 'bg-foreground/60'}`}
				style={`width: ${Math.max(2, contextPct)}%`}
			></div>
		</div>
		<span class="text-xs text-muted-foreground">
			{contextTokens > 0 ? `${contextTokens.toLocaleString()} of ${session.context_window.toLocaleString()} tokens` : 'No usage yet'}
		</span>
	</div>
</aside>
