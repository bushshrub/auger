<script>
	import { tick } from 'svelte';
	import 'katex/dist/katex.min.css';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { listSessions, sendInput, approveTool, subscribeEvents, getSnapshot } from '$lib/api.js';
	import { renderMarkdown } from '$lib/markdown.js';

	const sessionId = $derived(page.params.id);

	let session = $state(/** @type {import('$lib/api.js').SessionCreds | null} */ (null));
	let status = $state('connecting');
	let loadError = $state(/** @type {string|null} */ (null));

	/**
	 * @typedef {{ tokens?: number|null, ttftMs?: number|null, tps?: number|null }} MsgMeta
	 * @typedef {{ kind: 'user'|'assistant'|'reasoning'|'tool'|'error', text?: string,
	 *   toolName?: string, toolId?: string, args?: any, result?: any,
	 *   decided?: 'approved'|'denied', meta?: MsgMeta }} ChatItem
	 */
	let items = $state(/** @type {ChatItem[]} */ ([]));
	/** @type {Set<string>} */
	let pendingToolIds = $state(new Set());

	let ctxUsed = $state(0);
	let ctxWindow = $state(0);

	let assistantIdx = -1;
	let reasoningIdx = -1;
	/** @type {AbortController | null} */
	let sub = null;
	let draft = $state('');
	let log = $state(/** @type {HTMLDivElement | undefined} */ (undefined));

	$effect(() => {
		const id = sessionId;
		listSessions().then(async ({ sessions }) => {
			const info = sessions.find((s) => s.session_id === id);
			if (!info) {
				loadError = `Session ${id} not found`;
				status = 'error';
				return;
			}
			session = {
				session_id: info.session_id,
				owner_token: info.owner_token,
				viewer_token: info.viewer_token,
				context_window: info.context_window
			};
			ctxWindow = info.context_window;

			try {
				const snap = await getSnapshot(info.session_id, info.owner_token);
				const { items: snapItems, pending } = snapshotToItems(snap.messages);
				items = snapItems;
				pendingToolIds = pending;
				await scroll();
			} catch {
				// non-fatal: show empty history and continue
			}

			sub = subscribeEvents(info.session_id, info.owner_token, onEvent, (err) => {
				items.push({ kind: 'error', text: `stream: ${err.message}` });
				status = 'error';
			});
			status = pendingToolIds.size > 0 ? 'running' : 'idle';
		}).catch((err) => {
			loadError = String(err);
			status = 'error';
		});

		return () => {
			sub?.abort();
			sub = null;
		};
	});

	async function scroll() {
		await tick();
		log?.scrollTo({ top: log.scrollHeight });
	}

	/**
	 * @param {any[]} messages  SnapshotMessage array from server
	 * @returns {{ items: ChatItem[], pending: Set<string> }}
	 */
	function snapshotToItems(messages) {
		/** @type {ChatItem[]} */
		const result = [];
		/** @type {Record<string, number>} */
		const toolIdxMap = {};
		// Tool IDs from the most recent assistant block; reset whenever any message follows.
		let lastBlockIds = /** @type {string[]} */ ([]);

		for (const msg of messages) {
			if (msg.type === 'user') {
				lastBlockIds = [];
				result.push({ kind: 'user', text: msg.text });
			} else if (msg.type === 'assistant') {
				lastBlockIds = [];
				if (msg.reasoning) result.push({ kind: 'reasoning', text: msg.reasoning });
				if (msg.content) result.push({ kind: 'assistant', text: msg.content });
				for (const tc of msg.tool_calls ?? []) {
					let args = tc.arguments;
					try { args = JSON.parse(args); } catch { /* keep as string */ }
					toolIdxMap[tc.id] = result.length;
					lastBlockIds.push(tc.id);
					result.push({ kind: 'tool', toolId: tc.id, toolName: tc.name, args });
				}
			} else if (msg.type === 'tool') {
				const idx = toolIdxMap[msg.tool_call_id];
				if (idx !== undefined) result[idx].result = msg.content;
			}
		}

		// Tool calls followed by more messages were processed — mark done.
		// Tool calls still in lastBlockIds had no follow-up — still pending.
		const pending = new Set(lastBlockIds);
		for (const [id, idx] of Object.entries(toolIdxMap)) {
			if (!pending.has(id)) result[idx].decided = 'approved';
		}

		return { items: result, pending };
	}

	/** @param {any} ev */
	function onEvent(ev) {
		switch (ev.type) {
			case 'reasoning': {
				if (reasoningIdx === -1) {
					items.push({ kind: 'reasoning', text: '' });
					reasoningIdx = items.length - 1;
				}
				items[reasoningIdx].text = (items[reasoningIdx].text ?? '') + ev.data.text;
				break;
			}
			case 'content': {
				reasoningIdx = -1;
				if (assistantIdx === -1) {
					items.push({ kind: 'assistant', text: '' });
					assistantIdx = items.length - 1;
				}
				items[assistantIdx].text = (items[assistantIdx].text ?? '') + ev.data.text;
				ctxUsed += approxTokens(ev.data.text);
				break;
			}
			case 'tool_call':
				assistantIdx = -1;
				reasoningIdx = -1;
				items.push({
					kind: 'tool',
					toolId: ev.data.id,
					toolName: ev.data.name,
					args: ev.data.arguments
				});
				pendingToolIds = new Set([...pendingToolIds, ev.data.id]);
				status = 'running';
				break;
			case 'tool_result': {
				const t = items.find((i) => i.kind === 'tool' && i.toolId === ev.data.id);
				if (t) t.result = ev.data.content;
				assistantIdx = -1;
				break;
			}
			case 'metrics': {
				const m = ev.data;
				if (m.context_window) ctxWindow = m.context_window;
				if (m.total_tokens != null) ctxUsed = m.total_tokens;
				else if (m.prompt_tokens != null && m.completion_tokens != null)
					ctxUsed = m.prompt_tokens + m.completion_tokens;
				if (assistantIdx !== -1) {
					items[assistantIdx].meta = {
						tokens: m.completion_tokens,
						ttftMs: m.ttft_ms,
						tps: m.tokens_per_sec
					};
				}
				break;
			}
			case 'turn_complete':
				assistantIdx = -1;
				reasoningIdx = -1;
				status = 'idle';
				break;
			case 'error':
				items.push({ kind: 'error', text: ev.data.message });
				assistantIdx = -1;
				reasoningIdx = -1;
				pendingToolIds = new Set();
				status = 'idle';
				break;
		}
		scroll();
	}

	async function send() {
		const text = draft.trim();
		if (!text || !session || status !== 'idle') return;
		draft = '';
		items.push({ kind: 'user', text });
		ctxUsed += approxTokens(text);
		assistantIdx = -1;
		status = 'running';
		scroll();
		try {
			await sendInput(session.session_id, session.owner_token, text);
		} catch (err) {
			items.push({ kind: 'error', text: String(err) });
			status = 'idle';
		}
	}

	/**
	 * @param {string} toolId
	 * @param {boolean} approved
	 */
	async function decide(toolId, approved) {
		if (!session) return;
		const t = items.find((i) => i.kind === 'tool' && i.toolId === toolId);
		if (t) t.decided = approved ? 'approved' : 'denied';
		pendingToolIds = new Set([...pendingToolIds].filter((id) => id !== toolId));
		try {
			await approveTool(session.session_id, session.owner_token, toolId, approved);
		} catch (err) {
			items.push({ kind: 'error', text: String(err) });
		}
	}

	/** @param {KeyboardEvent} e */
	function onKey(e) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			send();
		}
	}

	/** @param {any} v */
	const pretty = (v) => (typeof v === 'string' ? v : JSON.stringify(v, null, 2));

	/** @param {number} n */
	const fmtInt = (n) => n.toLocaleString('en-US');

	const approxTokens = (/** @type {string} */ s) => Math.ceil(s.length / 4);

	let ctxPct = $derived(ctxWindow > 0 ? Math.min(100, (ctxUsed / ctxWindow) * 100) : 0);

	/** @param {MsgMeta} m */
	function metaLine(m) {
		const parts = [];
		if (m.tokens != null) parts.push(`${fmtInt(m.tokens)} tok`);
		if (m.tps != null) parts.push(`${m.tps.toFixed(1)} tok/s`);
		if (m.ttftMs != null) parts.push(`TTFT ${fmtInt(Math.round(m.ttftMs))} ms`);
		return parts.join(' · ');
	}
</script>

<div class="app">
	<header>
		<strong>auger</strong>
		<button class="back" onclick={() => goto('/')}>← Sessions</button>
		<span class="status status-{status}">{status}</span>
		<span class="sid">{sessionId.slice(0, 8)}</span>
	</header>

	{#if loadError}
		<div class="load-error">⚠ {loadError} — <a href="/">back to sessions</a></div>
	{:else}
		<div class="ctx" title="{fmtInt(ctxUsed)} / {ctxWindow ? fmtInt(ctxWindow) : '?'} context tokens">
			<div class="ctx-meter" class:warn={ctxPct >= 75} class:full={ctxPct >= 90}>
				<div class="ctx-fill" style="width: {ctxPct}%"></div>
			</div>
			<span class="ctx-label">
				{fmtInt(ctxUsed)} / {ctxWindow ? fmtInt(ctxWindow) : '—'} tok ({Math.round(ctxPct)}%)
			</span>
		</div>
		<div class="log" bind:this={log}>
			{#each items as item}
				{#if item.kind === 'user'}
					<div class="msg user"><div class="bubble">{item.text}</div></div>
				{:else if item.kind === 'assistant'}
					<div class="msg assistant">
						<!-- eslint-disable-next-line svelte/no-at-html-tags -->
						<div class="bubble md">{@html renderMarkdown(item.text)}</div>
						{#if item.meta}<div class="meta">{metaLine(item.meta)}</div>{/if}
					</div>
				{:else if item.kind === 'reasoning'}
					<div class="msg reasoning">
						<details>
							<summary>Thinking</summary>
							<!-- eslint-disable-next-line svelte/no-at-html-tags -->
							<div class="bubble md reasoning-body">{@html renderMarkdown(item.text)}</div>
						</details>
					</div>
				{:else if item.kind === 'error'}
					<div class="msg error"><div class="bubble">⚠ {item.text}</div></div>
				{:else if item.kind === 'tool'}
					<div class="msg tool">
						<div class="bubble">
							<div class="tool-head">
								🔧 <strong>{item.toolName}</strong>
								{#if item.decided}
									<span class="tag {item.decided}">{item.decided}</span>
								{:else if pendingToolIds.has(item.toolId)}
									<button class="ok inline-btn" onclick={() => decide(item.toolId, true)}>Approve</button>
									<button class="no inline-btn" onclick={() => decide(item.toolId, false)}>Deny</button>
								{/if}
							</div>
							<pre class="args">{pretty(item.args)}</pre>
							{#if item.result !== undefined}
								<div class="tool-sub">result</div>
								<pre class="result">{pretty(item.result)}</pre>
							{/if}
						</div>
					</div>
				{/if}
			{/each}
		</div>

		<div class="composer">
			<textarea
				rows="2"
				placeholder={status === 'idle' ? 'Message the agent… (Enter to send)' : 'agent is busy…'}
				bind:value={draft}
				onkeydown={onKey}
				disabled={status !== 'idle'}
			></textarea>
			<button onclick={send} disabled={status !== 'idle' || !draft.trim()}>Send</button>
		</div>
	{/if}
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
	.back {
		font-size: 0.82rem;
		padding: 0.2rem 0.6rem;
		background: none;
		border-color: var(--border);
		color: var(--muted);
	}
	.back:hover {
		color: var(--text);
	}
	.status {
		font-size: 0.78rem;
		padding: 0.1rem 0.5rem;
		border-radius: 999px;
		border: 1px solid var(--border);
		color: var(--muted);
	}
	.status-idle {
		color: #6fd08c;
		border-color: #2f5a3c;
	}
	.status-running,
	.status-connecting {
		color: var(--accent);
		border-color: #2c447f;
	}
	.status-error {
		color: var(--error);
		border-color: #5a2f2f;
	}
	.sid {
		margin-left: auto;
		color: var(--muted);
		font-family: ui-monospace, monospace;
		font-size: 0.78rem;
	}
	.load-error {
		padding: 2rem 1rem;
		color: var(--error);
		text-align: center;
	}
	.load-error a {
		color: var(--accent);
	}
	.ctx {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.4rem 1rem;
		border-bottom: 1px solid var(--border);
	}
	.ctx-meter {
		flex: 1;
		height: 6px;
		border-radius: 999px;
		background: var(--border);
		overflow: hidden;
	}
	.ctx-fill {
		height: 100%;
		background: var(--accent);
		transition: width 0.3s ease;
	}
	.ctx-meter.warn .ctx-fill {
		background: #e0b341;
	}
	.ctx-meter.full .ctx-fill {
		background: var(--error);
	}
	.ctx-label {
		color: var(--muted);
		font-size: 0.74rem;
		font-family: ui-monospace, monospace;
		white-space: nowrap;
	}
	.meta {
		margin-top: 0.25rem;
		color: var(--muted);
		font-size: 0.72rem;
		font-family: ui-monospace, monospace;
	}
	.log {
		flex: 1;
		overflow-y: auto;
		padding: 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.7rem;
	}
	.msg {
		display: flex;
	}
	.msg.user {
		justify-content: flex-end;
	}
	.msg.assistant {
		flex-direction: column;
		align-items: flex-start;
	}
	.bubble {
		max-width: 80%;
		padding: 0.6rem 0.8rem;
		border-radius: 10px;
		background: var(--panel);
		border: 1px solid var(--border);
		white-space: pre-wrap;
		word-break: break-word;
		line-height: 1.45;
	}
	.user .bubble {
		background: var(--user);
		border-color: #2c447f;
	}
	.bubble.md {
		white-space: normal;
	}
	.bubble.md :global(:first-child) { margin-top: 0; }
	.bubble.md :global(:last-child) { margin-bottom: 0; }
	.bubble.md :global(p) { margin: 0.5rem 0; }
	.bubble.md :global(h1),
	.bubble.md :global(h2),
	.bubble.md :global(h3),
	.bubble.md :global(h4) { margin: 0.9rem 0 0.4rem; line-height: 1.25; }
	.bubble.md :global(ul),
	.bubble.md :global(ol) { margin: 0.5rem 0; padding-left: 1.4rem; }
	.bubble.md :global(li) { margin: 0.2rem 0; }
	.bubble.md :global(a) { color: var(--accent); }
	.bubble.md :global(code) {
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 0.05rem 0.3rem;
		font-family: ui-monospace, monospace;
		font-size: 0.86em;
	}
	.bubble.md :global(pre) {
		margin: 0.6rem 0;
		padding: 0.7rem;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow-x: auto;
	}
	.bubble.md :global(pre code) { background: none; border: none; padding: 0; font-size: 0.84em; }
	.bubble.md :global(blockquote) {
		margin: 0.6rem 0;
		padding: 0.1rem 0.9rem;
		border-left: 3px solid var(--border);
		color: var(--muted);
	}
	.bubble.md :global(table) { border-collapse: collapse; margin: 0.6rem 0; }
	.bubble.md :global(th),
	.bubble.md :global(td) { border: 1px solid var(--border); padding: 0.35rem 0.6rem; }
	.bubble.md :global(hr) { border: none; border-top: 1px solid var(--border); margin: 0.9rem 0; }
	.bubble.md :global(.katex-display) { margin: 0.7rem 0; overflow-x: auto; overflow-y: hidden; }
	.msg.reasoning {
		align-items: flex-start;
		width: 100%;
	}
	.msg.reasoning details {
		width: 100%;
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 0.4rem 0.7rem;
		background: var(--panel);
	}
	.msg.reasoning summary {
		cursor: pointer;
		color: var(--muted);
		font-size: 0.78rem;
		font-style: italic;
		user-select: none;
	}
	.reasoning-body {
		margin-top: 0.5rem;
		color: var(--muted);
		font-size: 0.82rem;
		background: none;
		border: none;
		padding: 0;
		max-width: 100%;
	}
	.error .bubble {
		border-color: #5a2f2f;
		color: var(--error);
	}
	.tool .bubble {
		background: var(--tool);
		max-width: 100%;
		width: 100%;
	}
	.tool-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.tool-sub {
		margin-top: 0.5rem;
		color: var(--muted);
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	pre {
		margin: 0.4rem 0 0;
		padding: 0.5rem;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow-x: auto;
		font-size: 0.82rem;
		font-family: ui-monospace, monospace;
	}
	.tag {
		font-size: 0.72rem;
		padding: 0.05rem 0.4rem;
		border-radius: 999px;
	}
	.tag.approved { background: #1f3a28; color: #6fd08c; }
	.tag.denied { background: #3a1f1f; color: var(--error); }
	.inline-btn {
		font-size: 0.72rem;
		padding: 0.1rem 0.5rem;
	}
	.ok { border-color: #2f5a3c; }
	.no { border-color: #5a2f2f; }
	.composer {
		display: flex;
		gap: 0.6rem;
		padding: 0.8rem 1rem;
		border-top: 1px solid var(--border);
	}
	.composer textarea {
		flex: 1;
		resize: none;
	}
</style>
