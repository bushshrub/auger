<script>
	import { AugerSession } from '$lib/session.svelte.js';
	import AssistantMessage from './AssistantMessage.svelte';
	import Composer from './Composer.svelte';
	import ToolCallCard from './ToolCallCard.svelte';
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

	const totalTokens = $derived(
		agent.totalUsage.prompt_tokens + agent.totalUsage.completion_tokens
	);
	const contextPct = $derived(
		Math.min(100, Math.round((agent.contextTokens / session.context_window) * 100))
	);
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<!-- Status bar -->
	<header class="flex items-center gap-3 border-b border-border px-4 py-2">
		<span
			class={`size-2 rounded-full ${agent.connected ? 'bg-success' : 'bg-destructive'}`}
			role="status"
			aria-label={agent.connected ? 'Connected' : 'Disconnected'}
		></span>
		<span class="font-mono text-xs text-foreground">{session.model}</span>
		<span class="font-mono text-[10px] text-muted-foreground">
			{session.session_id.slice(0, 8)}
		</span>
		<div class="ml-auto flex items-center gap-3">
			<span class="hidden font-mono text-[10px] text-muted-foreground sm:inline">
				{totalTokens > 0 ? `${totalTokens.toLocaleString()} tokens total` : 'no usage yet'}
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

			{#each agent.items as item (item.id)}
				{#if item.kind === 'user'}
					<UserMessage text={item.text} />
				{:else if item.kind === 'assistant'}
					<AssistantMessage {item} />
				{:else}
					<div class="pl-7">
						<ToolCallCard call={item.call} onRespond={(id, ok, msg) => agent.respond(id, ok, msg)} />
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
			<Composer
				busy={agent.busy}
				onSend={(text) => agent.send(text)}
				onInterrupt={() => agent.interrupt()}
			/>
		</div>
	</div>
</div>
