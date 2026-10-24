<script>
	import { AugerSession, GROUPABLE_TOOLS } from '$lib/session.svelte.js';
	import AssistantMessage from './AssistantMessage.svelte';
	import Composer from './Composer.svelte';
	import ToolCallCard from './ToolCallCard.svelte';
	import ToolCallGroup from './ToolCallGroup.svelte';
	import TodoPanel from './TodoPanel.svelte';
	import UserMessage from './UserMessage.svelte';

	/** @type {{ session: import('$lib/api.js').SessionInfo }} */
	let { session } = $props();

	// The parent recreates this component per session id ({#key}), so one
	// AugerSession instance lives exactly as long as the view and the initial
	// capture of `session` is intentional.
	// svelte-ignore state_referenced_locally
	const agent = new AugerSession(session.session_id, session.tokens);

	$effect(() => {
		agent.start();
		return () => agent.stop();
	});

	/** @type {HTMLDivElement | undefined} */
	let scrollEl = $state();
	let pinned = true;

	function onscroll() {
		if (!scrollEl) return;
		pinned = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 80;
	}

	// Keep the transcript pinned to the bottom while new content streams in,
	// unless the user has scrolled up. Reading the streamed fields registers
	// them as dependencies so the effect re-runs on every delta.
	$effect(() => {
		for (const item of agent.items) {
			if (item.kind === 'assistant') {
				void item.content;
				void item.reasoning;
				void item.streaming;
			} else if (item.kind === 'tool') {
				void item.call.status;
				void item.call.result;
				void item.call.error;
			}
		}
		if (scrollEl && pinned) {
			scrollEl.scrollTop = scrollEl.scrollHeight;
		}
	});

	/**
	 * @typedef {import('$lib/session.svelte.js').UiItem} UiItem
	 * @typedef {import('$lib/session.svelte.js').UiToolCall} UiToolCall
	 * @typedef {{ kind: 'reasoning', id: string, text: string }
	 *         | { kind: 'tool', id: string, call: UiToolCall }} GroupEntry
	 * @typedef {{ kind: 'item', id: string, item: UiItem }
	 *         | { kind: 'group', id: string, entries: GroupEntry[] }} Row
	 */

	/** A read-only tool call that folds into a group. @param {UiItem} it */
	const isGroupableTool = (it) => it.kind === 'tool' && GROUPABLE_TOOLS.has(it.call.name);
	// A finished assistant turn that only carried reasoning (its content, if any,
	// arrives on the final turn). These sit between the tool calls of an
	// exploration burst, so they fold into the group to keep the step order.
	/** @param {UiItem} it */
	const isFoldableReasoning = (it) =>
		it.kind === 'assistant' &&
		!it.streaming &&
		it.content.trim().length === 0 &&
		it.reasoning.trim().length > 0;

	// Collapse runs of consecutive read-only tool calls — and the reasoning-only
	// turns interleaved with them — into a single group card so exploration
	// bursts don't flood the transcript. Requires 2+ tool calls to group; a lone
	// call (with its reasoning) renders inline as before.
	const rows = $derived.by(() => {
		/** @type {Row[]} */
		const out = [];
		/** @type {UiItem[]} */
		const items = agent.items;
		let i = 0;
		while (i < items.length) {
			const item = items[i];
			if (isGroupableTool(item) || isFoldableReasoning(item)) {
				/** @type {UiItem[]} */
				const run = [];
				while (i < items.length && (isGroupableTool(items[i]) || isFoldableReasoning(items[i]))) {
					run.push(items[i]);
					i++;
				}
				if (run.filter(isGroupableTool).length >= 2) {
					out.push({
						kind: 'group',
						id: `group:${run[0].id}`,
						entries: run.map((r) =>
							r.kind === 'tool'
								? { kind: 'tool', id: r.id, call: r.call }
								: { kind: 'reasoning', id: r.id, text: /** @type {any} */ (r).reasoning }
						)
					});
					continue;
				}
				for (const r of run) out.push({ kind: 'item', id: r.id, item: r });
				continue;
			}
			out.push({ kind: 'item', id: item.id, item });
			i++;
		}
		return out;
	});

	const contextPct = $derived(
		Math.min(100, Math.round((agent.contextTokens / session.context_window) * 100))
	);
</script>

<div class="flex min-h-0 flex-1">
	<div class="flex min-h-0 min-w-0 flex-1 flex-col">
	<!-- Status bar -->
	<header class="flex items-center gap-3 border-b border-border px-4 py-2">
		{#if session.archived}
			<span
				class="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
			>archived</span>
		{:else}
			<span
				class={`size-2 rounded-full ${agent.connected ? 'bg-success' : 'bg-destructive'}`}
				role="status"
				aria-label={agent.connected ? 'Connected' : 'Disconnected'}
			></span>
		{/if}
		<span class="font-mono text-xs text-foreground">{session.model}</span>
		<span class="font-mono text-[10px] text-muted-foreground">
			{session.session_id.slice(0, 8)}
		</span>
		<div class="ml-auto flex items-center gap-3">
			<span class="hidden font-mono text-[10px] text-muted-foreground sm:inline">
				{agent.contextTokens > 0 ? `${agent.contextTokens.toLocaleString()} tokens in context` : 'no usage yet'}
			</span>
			<div class="flex items-center gap-1.5" title={`Context: ${contextPct}%`}>
				<div class="h-1.5 w-16 overflow-hidden rounded-full bg-muted">
					<div
						class={`h-full rounded-full ${contextPct > 80 ? 'bg-destructive' : 'bg-primary'}`}
						style={`width: ${Math.max(2, contextPct)}%`}
					></div>
				</div>
				<span class="font-mono text-[10px] text-muted-foreground">{contextPct}%</span>
			</div>
		</div>
	</header>

	<!-- Transcript -->
	<div bind:this={scrollEl} {onscroll} class="min-h-0 flex-1 overflow-y-auto auger-scroll">
		<div class="mx-auto flex w-full max-w-3xl flex-col gap-5 px-4 py-6">
			{#if agent.items.length === 0}
				<div class="flex flex-col gap-2 rounded-lg border border-border bg-card p-5">
					<p class="font-mono text-sm text-foreground">session ready — {session.model}</p>
					<p class="text-sm leading-relaxed text-muted-foreground">
						Ask the agent to explore the repo, fix a bug, or run commands. Read-only tools run
						automatically; shell, edit, and write wait for your approval.
					</p>
					<p class="font-mono text-xs text-muted-foreground">
						try: "find and fix the bug in the rate limiter"
					</p>
				</div>
			{/if}

			{#each rows as row (row.id)}
				{#if row.kind === 'group'}
					<div class="pl-7">
						<ToolCallGroup
							entries={row.entries}
							sessionArchived={session.archived}
							onRespond={(id, ok, msg) => agent.respond(id, ok, msg)}
						/>
					</div>
				{:else if row.item.kind === 'user'}
					<UserMessage text={row.item.text} />
				{:else if row.item.kind === 'assistant'}
					<AssistantMessage item={row.item} />
				{:else}
					<div class="pl-7">
						<ToolCallCard
							call={row.item.call}
							sessionArchived={session.archived}
							onRespond={(id, ok, msg) => agent.respond(id, ok, msg)}
						/>
					</div>
				{/if}
			{/each}

			{#if agent.error}
				<p class="rounded-md bg-destructive/10 px-3 py-2 font-mono text-xs text-destructive">
					{agent.error}
				</p>
			{/if}
		</div>
	</div>

	<!-- Composer -->
	<div class="border-t border-border px-4 py-3">
		<div class="mx-auto w-full max-w-3xl">
			{#if session.archived}
				<p class="py-2 text-center font-mono text-xs text-muted-foreground">
					this session is archived and cannot accept new messages
				</p>
			{:else}
				<Composer
					busy={agent.busy}
					onSend={(text) => agent.send(text)}
					onInterrupt={() => agent.interrupt()}
				/>
			{/if}
		</div>
	</div>
	</div>

	{#if agent.todos.length > 0}
		<TodoPanel todos={agent.todos} />
	{/if}
</div>
