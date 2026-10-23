<script>
	import { AugerSession } from '$lib/session.svelte.js';
	import { sessionMeta, sessionTitle } from '$lib/session-meta.svelte.js';
	import Composer from './Composer.svelte';
	import RightPanel from './RightPanel.svelte';
	import SessionNotices from './SessionNotices.svelte';
	import TodoPanel from './TodoPanel.svelte';
	import TopBar from './TopBar.svelte';
	import Transcript from './Transcript.svelte';

	/**
	 * @type {{
	 *   session: import('$lib/api.js').SessionInfo,
	 *   sidebarOpen: boolean,
	 *   onToggleSidebar: () => void,
	 *   onArchive: (id: string) => void
	 * }}
	 */
	let { session, sidebarOpen, onToggleSidebar, onArchive } = $props();

	// The parent recreates this component per session id ({#key}), so one
	// AugerSession instance lives exactly as long as the view.
	// svelte-ignore state_referenced_locally
	const agent = new AugerSession(session.session_id, session.tokens);
	// svelte-ignore state_referenced_locally
	const id = session.session_id;

	$effect(() => {
		agent.start();
		return () => agent.stop();
	});

	let infoOpen = $state(false);

	const firstUser = $derived(agent.items.find((i) => i.kind === 'user'));
	const firstMessage = $derived(
		firstUser?.kind === 'user' ? firstUser.text.replace(/\s+/g, ' ').trim().slice(0, 60) : ''
	);
	const approval = $derived(
		agent.items.some((i) => i.kind === 'tool' && i.call.status === 'pending_approval')
	);

	// Sidebar state for this session: title fallback and activity. Cleared on
	// unmount so a session left behind does not read as busy.
	sessionMeta[id] ??= {};
	$effect(() => {
		sessionMeta[id].firstMessage = firstMessage;
	});
	$effect(() => {
		const meta = sessionMeta[id];
		meta.busy = agent.busy;
		meta.approval = approval;
		return () => {
			meta.busy = false;
			meta.approval = false;
		};
	});
</script>

<div class="flex min-h-0 min-w-0 flex-1">
	<div class="flex min-h-0 min-w-0 flex-1 flex-col">
		<TopBar
			{id}
			title={sessionTitle(id)}
			model={session.model}
			archived={session.archived}
			connected={agent.connected}
			{sidebarOpen}
			{infoOpen}
			{onToggleSidebar}
			onToggleInfo={() => (infoOpen = !infoOpen)}
			onArchive={() => onArchive(id)}
		/>

		<Transcript
			items={agent.items}
			sessionArchived={session.archived}
			onRespond={(toolCallId, ok, msg) => agent.respond(toolCallId, ok, msg)}
		/>

		<div class="shrink-0 px-4 pt-2 pb-4">
			<div class="mx-auto flex w-full max-w-3xl flex-col gap-2">
				<SessionNotices error={agent.error} toolsMissing={agent.toolsMissing} />
				{#if session.archived}
					<p class="py-2 text-center text-xs text-muted-foreground">
						This session is archived and cannot accept new messages.
					</p>
				{:else}
					<Composer
						busy={agent.busy}
						model={session.model}
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
	{#if infoOpen}
		<RightPanel {session} contextTokens={agent.contextTokens} connected={agent.connected} />
	{/if}
</div>
