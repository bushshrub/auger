<script>
	import { goto } from '$app/navigation';
	import { listSessions, createSession } from '$lib/api.js';

	let sessionList = $state(/** @type {import('$lib/api.js').SessionInfo[]} */ ([]));
	let browserLoading = $state(false);
	let browserError = $state(/** @type {string|null} */ (null));
	let model = $state('');
	let connecting = $state(false);

	$effect(() => {
		loadSessions();
		const interval = setInterval(loadSessions, 5000);
		return () => clearInterval(interval);
	});

	async function loadSessions() {
		browserLoading = true;
		browserError = null;
		try {
			const result = await listSessions();
			sessionList = result.sessions.slice().sort((a, b) => b.created_at - a.created_at);
		} catch (err) {
			browserError = String(err);
		} finally {
			browserLoading = false;
		}
	}

	async function connect() {
		connecting = true;
		try {
			const session = await createSession(model.trim() || undefined);
			goto(`/sessions/${session.session_id}`);
		} catch (err) {
			browserError = String(err);
			connecting = false;
		}
	}

	/** @param {number} ts unix seconds */
	function relTime(ts) {
		const diff = Math.floor(Date.now() / 1000) - ts;
		if (diff < 60) return `${diff}s ago`;
		if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
		if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
		return `${Math.floor(diff / 86400)}d ago`;
	}
</script>

<div class="app">
	<header>
		<strong>auger</strong>
	</header>

	<div class="browser">
		<div class="browser-header">
			<span class="browser-title">Sessions</span>
			<div class="new-session-form">
				<input
					class="model-input"
					placeholder="model (optional)"
					bind:value={model}
					onkeydown={(e) => e.key === 'Enter' && connect()}
				/>
				<button onclick={connect} disabled={connecting}>
					{connecting ? 'connecting…' : '+ New'}
				</button>
			</div>
		</div>

		{#if browserError}
			<div class="browser-empty error-text">⚠ {browserError}</div>
		{:else if browserLoading && sessionList.length === 0}
			<div class="browser-empty">Loading…</div>
		{:else if sessionList.length === 0}
			<div class="browser-empty">No active sessions. Create one above.</div>
		{:else}
			<ul class="session-list">
				{#each sessionList as info}
					<li class="session-row">
						<div class="session-model">{info.model}</div>
						<div class="session-meta">
							<span class="session-id">{info.session_id.slice(0, 8)}</span>
							<span class="session-age">{relTime(info.created_at)}</span>
						</div>
						<button class="open-btn" onclick={() => goto(`/sessions/${info.session_id}`)}>Open →</button>
					</li>
				{/each}
			</ul>
		{/if}

		<div class="browser-refresh">
			<button class="ghost" onclick={loadSessions} disabled={browserLoading}>Refresh</button>
		</div>
	</div>
</div>

<style>
	.app {
		display: flex;
		flex-direction: column;
		height: 100vh;
		max-width: 860px;
		margin: 0 auto;
	}
	header {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.7rem 1rem;
		border-bottom: 1px solid var(--border);
	}
	.browser {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}
	.browser-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.9rem 1rem 0.6rem;
		border-bottom: 1px solid var(--border);
	}
	.browser-title {
		font-size: 0.88rem;
		font-weight: 600;
		color: var(--muted);
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}
	.new-session-form {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.model-input {
		width: 180px;
		font-size: 0.84rem;
	}
	.browser-empty {
		padding: 2rem 1rem;
		color: var(--muted);
		font-size: 0.88rem;
		text-align: center;
	}
	.error-text {
		color: var(--error);
	}
	.session-list {
		list-style: none;
		margin: 0;
		padding: 0.5rem 0;
		overflow-y: auto;
		flex: 1;
	}
	.session-row {
		display: flex;
		align-items: center;
		gap: 0.8rem;
		padding: 0.7rem 1rem;
		border-bottom: 1px solid var(--border);
		cursor: pointer;
		transition: background 0.1s;
	}
	.session-row:hover {
		background: var(--panel);
	}
	.session-model {
		font-family: ui-monospace, monospace;
		font-size: 0.84rem;
		flex: 1;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.session-meta {
		display: flex;
		gap: 0.8rem;
		color: var(--muted);
		font-size: 0.78rem;
		font-family: ui-monospace, monospace;
		white-space: nowrap;
	}
	.session-id {
		color: var(--muted);
	}
	.session-age {
		color: var(--muted);
	}
	.open-btn {
		font-size: 0.78rem;
		padding: 0.2rem 0.6rem;
		border-color: var(--border);
		color: var(--muted);
		background: none;
		white-space: nowrap;
	}
	.open-btn:hover {
		color: var(--accent);
		border-color: var(--accent);
	}
	.browser-refresh {
		padding: 0.5rem 1rem;
		border-top: 1px solid var(--border);
	}
	.ghost {
		background: none;
		font-size: 0.8rem;
		color: var(--muted);
		border-color: var(--border);
	}
</style>
