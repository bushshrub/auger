<script>
	import { tick } from 'svelte';
	import 'katex/dist/katex.min.css';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { listSessions, sendInput, approveTool, subscribeEvents, getSnapshot } from '$lib/api.js';
	import { renderMarkdown } from '$lib/markdown.js';
	import DiffViewer from '$lib/DiffViewer.svelte';

	const sessionId = $derived(page.params.id);

	let session = $state(/** @type {import('$lib/api.js').SessionCreds | null} */ (null));
	let status = $state('connecting');
	let loadError = $state(/** @type {string|null} */ (null));

	/**
	 * @typedef {{ tokens?: number|null, ttftMs?: number|null, tps?: number|null }} MsgMeta
	 * @typedef {{ kind: 'user'|'assistant'|'reasoning'|'tool'|'error', text?: string,
	 *   toolName?: string, toolId?: string, args?: any, result?: any,
	 *   decided?: 'approved'|'denied'|'auto', meta?: MsgMeta }} ChatItem
	 */
	let items = $state(/** @type {ChatItem[]} */ ([]));
	/** @type {Set<string>} */
	let pendingToolIds = $state(new Set());

	let ctxUsed = $state(0);
	let ctxWindow = $state(0);
	/** @type {Record<string, string>} */
	let toolMessages = $state({});

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
				if (msg.text) result.push({ kind: 'user', text: msg.text });
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
			case 'tool_call_auto_approved': {
				assistantIdx = -1;
				reasoningIdx = -1;
				const existing = items.find((i) => i.kind === 'tool' && i.toolId === ev.data.id);
				if (existing) {
					existing.decided = 'auto';
					pendingToolIds = new Set([...pendingToolIds].filter((id) => id !== ev.data.id));
				} else {
					items.push({
						kind: 'tool',
						toolId: ev.data.id,
						toolName: ev.data.name,
						args: ev.data.arguments,
						decided: 'auto'
					});
				}
				status = 'running';
				break;
			}
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
		const message = toolMessages[toolId]?.trim() || undefined;
		delete toolMessages[toolId];
		const t = items.find((i) => i.kind === 'tool' && i.toolId === toolId);
		if (t) t.decided = approved ? 'approved' : 'denied';
		pendingToolIds = new Set([...pendingToolIds].filter((id) => id !== toolId));
		try {
			await approveTool(session.session_id, session.owner_token, toolId, approved, message);
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

	/** @param {string} name */
	const isEditTool = (name) => name === 'edit_file';
	/** @param {string} name */
	const isWriteTool = (name) => name === 'write_file';
	/** @param {string} name */
	const isTodoTool = (name) => name === 'todo_list';
	/** @param {string} name */
	const isReadTool = (name) => name === 'read_file';
	/** @param {string} name */
	const isListTool = (name) => name === 'list_files';
	/** @param {string} name */
	const isGlobTool = (name) => name === 'glob';
	/** @param {string} name */
	const isGrepTool = (name) => name === 'grep';
	/** @param {string} name */
	const isShellTool = (name) => name === 'shell';

	/**
	 * Tokenize a shell command string into highlighted HTML spans.
	 * @param {string} cmd
	 * @returns {string}
	 */
	function highlightShell(cmd) {
		const esc = (/** @type {string} */ s) =>
			s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

		/** @type {{ cls: string, text: string }[]} */
		const tokens = [];
		let i = 0;
		let firstToken = true;

		while (i < cmd.length) {
			// comment
			if (cmd[i] === '#') {
				const end = cmd.indexOf('\n', i);
				const text = end === -1 ? cmd.slice(i) : cmd.slice(i, end + 1);
				tokens.push({ cls: 'sh-comment', text });
				i += text.length;
				continue;
			}
			// double-quoted string
			if (cmd[i] === '"') {
				let j = i + 1;
				while (j < cmd.length && cmd[j] !== '"') {
					if (cmd[j] === '\\') j++;
					j++;
				}
				tokens.push({ cls: 'sh-str', text: cmd.slice(i, j + 1) });
				i = j + 1;
				continue;
			}
			// single-quoted string
			if (cmd[i] === "'") {
				const end = cmd.indexOf("'", i + 1);
				const text = end === -1 ? cmd.slice(i) : cmd.slice(i, end + 1);
				tokens.push({ cls: 'sh-str', text });
				i += text.length;
				continue;
			}
			// variable: $(...), ${...}, $WORD
			if (cmd[i] === '$') {
				if (cmd[i + 1] === '(') {
					let depth = 1, j = i + 2;
					while (j < cmd.length && depth > 0) {
						if (cmd[j] === '(') depth++;
						else if (cmd[j] === ')') depth--;
						j++;
					}
					tokens.push({ cls: 'sh-var', text: cmd.slice(i, j) });
					i = j;
				} else if (cmd[i + 1] === '{') {
					const end = cmd.indexOf('}', i + 2);
					const text = end === -1 ? cmd.slice(i) : cmd.slice(i, end + 1);
					tokens.push({ cls: 'sh-var', text });
					i += text.length;
				} else {
					let j = i + 1;
					while (j < cmd.length && /\w/.test(cmd[j])) j++;
					tokens.push({ cls: 'sh-var', text: cmd.slice(i, j) });
					i = j;
				}
				continue;
			}
			// backtick subshell
			if (cmd[i] === '`') {
				const end = cmd.indexOf('`', i + 1);
				const text = end === -1 ? cmd.slice(i) : cmd.slice(i, end + 1);
				tokens.push({ cls: 'sh-var', text });
				i += text.length;
				continue;
			}
			// two-char operators
			const op2 = cmd.slice(i, i + 2);
			if (op2 === '&&' || op2 === '||' || op2 === '>>' || op2 === '>&' || op2 === ';;') {
				tokens.push({ cls: 'sh-op', text: op2 });
				i += 2;
				firstToken = true;
				continue;
			}
			// single-char operators
			if ('|;><&'.includes(cmd[i])) {
				tokens.push({ cls: 'sh-op', text: cmd[i] });
				i++;
				firstToken = true;
				continue;
			}
			// whitespace — reset firstToken after newline
			if (/\s/.test(cmd[i])) {
				let j = i;
				while (j < cmd.length && /\s/.test(cmd[j])) j++;
				const ws = cmd.slice(i, j);
				tokens.push({ cls: '', text: ws });
				if (ws.includes('\n')) firstToken = true;
				i = j;
				continue;
			}
			// word: command name, flag, or plain argument
			let j = i;
			while (j < cmd.length && !/[\s"'$`|;&><#]/.test(cmd[j])) j++;
			const word = cmd.slice(i, j);
			let cls = '';
			if (firstToken) {
				cls = 'sh-cmd';
				firstToken = false;
			} else if (/^-/.test(word)) {
				cls = 'sh-flag';
			}
			tokens.push({ cls, text: word });
			i = j;
		}

		return tokens
			.map((t) => (t.cls ? `<span class="${t.cls}">${esc(t.text)}</span>` : esc(t.text)))
			.join('');
	}

	/**
	 * @param {any} result
	 * @returns {{ stdout: string, stderr: string, exit_code: number } | null}
	 */
	function parseShellResult(result) {
		try {
			const parsed = typeof result === 'string' ? JSON.parse(result) : result;
			if (parsed && typeof parsed.exit_code === 'number') return parsed;
		} catch { /* ignore */ }
		return null;
	}

	/**
	 * @param {any} result
	 * @returns {{ id: number, title: string, status: string }[]}
	 */
	function parseTodoResult(result) {
		try {
			const parsed = typeof result === 'string' ? JSON.parse(result) : result;
			return Array.isArray(parsed?.items) ? parsed.items : [];
		} catch {
			return [];
		}
	}

	/** @param {string} status */
	function todoIcon(status) {
		if (status === 'done') return '●';
		if (status === 'in_progress') return '◑';
		return '○';
	}

	/** @param {any} args */
	function todoActionLabel(args) {
		const action = args?.action ?? '?';
		if (action === 'add') return `add: "${args?.title ?? ''}"`;
		if (action === 'update') {
			const parts = [];
			if (args?.title) parts.push(`title → "${args.title}"`);
			if (args?.status) parts.push(`status → ${args.status}`);
			return `update #${args?.id}: ${parts.join(', ')}`;
		}
		if (action === 'remove') return `remove #${args?.id}`;
		return action;
	}

	/** @param {number} n */
	const fmtInt = (n) => n.toLocaleString('en-US');

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
		<span class="sid">{sessionId?.slice(0, 8) ?? ''}</span>
	</header>

	{#if loadError}
		<div class="load-error">⚠ {loadError} — <a href="/">back to sessions</a></div>
	{:else}
		<div class="ctx" title="{fmtInt(ctxUsed)} / {ctxWindow ? fmtInt(ctxWindow) : '?'} context tokens">
			<div class="ctx-meter" class:warn={ctxPct >= 75} class:full={ctxPct >= 90} class:generating={status === 'running'}>
				<div class="ctx-fill" style="width: {ctxPct}%"></div>
			</div>
			<span class="ctx-label">
				{fmtInt(ctxUsed)} / {ctxWindow ? fmtInt(ctxWindow) : '—'} tok ({Math.round(ctxPct)}%){#if status === 'running'}<span class="ctx-tick" aria-hidden="true"></span>{/if}
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
				{:else if item.kind === 'tool' && item.toolName && (isReadTool(item.toolName) || isListTool(item.toolName) || isGlobTool(item.toolName) || isGrepTool(item.toolName))}
					<div class="msg tool-fs">
						<div class="tool-fs-row">
							{#if isReadTool(item.toolName)}
								<span class="tool-fs-verb">Reading</span>
								<span class="tool-fs-path">{item.args?.path ?? ''}</span>
							{:else if isListTool(item.toolName)}
								<span class="tool-fs-verb">Listing</span>
								<span class="tool-fs-path">{item.args?.path ?? ''}</span>
							{:else if isGlobTool(item.toolName)}
								<span class="tool-fs-verb">Glob</span>
								<span class="tool-fs-path">{item.args?.pattern ?? ''}</span>
							{:else if isGrepTool(item.toolName)}
								<span class="tool-fs-verb">Grep</span><span class="tool-fs-paren">(</span><span class="tool-fs-path">{item.args?.pattern ?? ''}</span><span class="tool-fs-paren">)</span>
							{/if}
							{#if item.decided}
								<span class="tag {item.decided}">{item.decided}</span>
							{:else if item.toolId != null && pendingToolIds.has(item.toolId)}
								{@const toolId = item.toolId}
								<input class="tool-msg-input" bind:value={toolMessages[toolId]} placeholder="optional message…" />
								<button class="ok inline-btn" onclick={() => decide(toolId, true)}>Approve</button>
								<button class="no inline-btn" onclick={() => decide(toolId, false)}>Deny</button>
							{/if}
						</div>
					</div>
				{:else if item.kind === 'tool'}
					<div class="msg tool">
						<div class="bubble">
							<div class="tool-head">
								🔧 <strong>{item.toolName}</strong>
								{#if item.decided}
									<span class="tag {item.decided}">{item.decided}</span>
								{:else if item.toolId != null && pendingToolIds.has(item.toolId)}
									{@const toolId = item.toolId}
									<input class="tool-msg-input" bind:value={toolMessages[toolId]} placeholder="optional message…" />
									<button class="ok inline-btn" onclick={() => decide(toolId, true)}>Approve</button>
									<button class="no inline-btn" onclick={() => decide(toolId, false)}>Deny</button>
								{/if}
							</div>
							{#if item.toolName && isEditTool(item.toolName) && item.args?.path}
								<div class="tool-path">{item.args.path}</div>
								<DiffViewer
									oldContent={item.args.old_string ?? ''}
									newContent={item.args.new_string ?? ''}
									fileName={item.args.path}
								/>
							{:else if item.toolName && isWriteTool(item.toolName) && item.args?.path}
								<div class="tool-path">{item.args.path}</div>
								<DiffViewer
									oldContent=""
									newContent={item.args.content ?? ''}
									fileName={item.args.path}
								/>
							{:else if item.toolName && isShellTool(item.toolName)}
								<!-- eslint-disable-next-line svelte/no-at-html-tags -->
								<pre class="shell-cmd">$ {@html highlightShell(item.args?.command ?? '')}</pre>
							{:else if item.toolName && isTodoTool(item.toolName)}
								<div class="todo-action">{todoActionLabel(item.args)}</div>
							{:else}
								<pre class="args">{pretty(item.args)}</pre>
							{/if}
							{#if item.result !== undefined}
								{#if item.toolName && isShellTool(item.toolName)}
									{@const sr = parseShellResult(item.result)}
									{#if sr}
										<div class="shell-result">
											<span class="shell-exit" class:exit-ok={sr.exit_code === 0} class:exit-err={sr.exit_code !== 0}>
												exit {sr.exit_code}
											</span>
										</div>
										{#if sr.stdout}
											<pre class="shell-out">{sr.stdout}</pre>
										{/if}
										{#if sr.stderr}
											<div class="tool-sub">stderr</div>
											<pre class="shell-err">{sr.stderr}</pre>
										{/if}
									{:else}
										<pre class="result">{pretty(item.result)}</pre>
									{/if}
								{:else if item.toolName && isTodoTool(item.toolName)}
									{@const todos = parseTodoResult(item.result)}
									{#if todos.length === 0}
										<div class="todo-empty">no items</div>
									{:else}
										<ul class="todo-items">
											{#each todos as todo}
												<li class="todo-item todo-{todo.status}">
													<span class="todo-icon">{todoIcon(todo.status)}</span>
													<span class="todo-title">{todo.title}</span>
													<span class="todo-id">#{todo.id}</span>
												</li>
											{/each}
										</ul>
									{/if}
								{:else}
									<div class="tool-sub">result</div>
									<pre class="result">{pretty(item.result)}</pre>
								{/if}
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
	.ctx-meter.generating .ctx-fill {
		background-image: linear-gradient(90deg, var(--accent) 0%, color-mix(in srgb, var(--accent) 60%, white) 50%, var(--accent) 100%);
		background-size: 200% 100%;
		animation: ctx-shimmer 1.4s linear infinite;
	}
	.ctx-meter.generating.warn .ctx-fill {
		background-image: linear-gradient(90deg, #e0b341 0%, color-mix(in srgb, #e0b341 60%, white) 50%, #e0b341 100%);
		background-size: 200% 100%;
	}
	.ctx-meter.generating.full .ctx-fill {
		background-image: linear-gradient(90deg, var(--error) 0%, color-mix(in srgb, var(--error) 60%, white) 50%, var(--error) 100%);
		background-size: 200% 100%;
	}
	@keyframes ctx-shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}
	.ctx-label {
		color: var(--muted);
		font-size: 0.74rem;
		font-family: ui-monospace, monospace;
		white-space: nowrap;
	}
	.ctx-tick::after {
		content: ' ·';
		animation: ctx-dots 1.2s steps(3, end) infinite;
		letter-spacing: 0.15em;
	}
	@keyframes ctx-dots {
		0%   { content: ' ·'; }
		33%  { content: ' ··'; }
		66%  { content: ' ···'; }
		100% { content: ' ·'; }
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
	.tag.auto { background: var(--panel); color: var(--muted); border: 1px solid var(--border); }
	.inline-btn {
		font-size: 0.72rem;
		padding: 0.1rem 0.5rem;
	}
	.tool-msg-input {
		font-size: 0.72rem;
		padding: 0.1rem 0.4rem;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 4px;
		color: var(--text);
		width: 12rem;
	}
	.tool-msg-input::placeholder {
		color: var(--muted);
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
	.tool-path {
		margin: 0.3rem 0 0.2rem;
		font-family: ui-monospace, monospace;
		font-size: 0.78rem;
		color: var(--muted);
	}
	.todo-action {
		margin: 0.3rem 0 0;
		font-family: ui-monospace, monospace;
		font-size: 0.82rem;
		color: var(--muted);
	}
	.todo-items {
		list-style: none;
		margin: 0.4rem 0 0;
		padding: 0.5rem;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 6px;
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
	}
	.todo-item {
		display: flex;
		align-items: baseline;
		gap: 0.5rem;
		font-size: 0.84rem;
		padding: 0.15rem 0;
	}
	.todo-icon {
		font-size: 0.9rem;
		flex-shrink: 0;
	}
	.todo-title {
		flex: 1;
	}
	.todo-id {
		font-family: ui-monospace, monospace;
		font-size: 0.72rem;
		color: var(--muted);
		flex-shrink: 0;
	}
	.todo-done .todo-title {
		text-decoration: line-through;
		color: var(--muted);
	}
	.todo-done .todo-icon {
		color: #6fd08c;
	}
	.todo-in_progress .todo-icon {
		color: var(--accent);
	}
	.todo-empty {
		margin: 0.3rem 0 0;
		font-size: 0.82rem;
		color: var(--muted);
		font-style: italic;
	}
	.msg.tool-fs {
		align-items: center;
	}
	.tool-fs-row {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		font-family: ui-monospace, monospace;
		font-size: 0.8rem;
		color: var(--muted);
		padding: 0.1rem 0;
	}
	.tool-fs-verb {
		color: var(--text);
		font-style: italic;
		flex-shrink: 0;
	}
	.tool-fs-path {
		color: var(--accent);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tool-fs-paren {
		color: var(--muted);
	}
	.shell-cmd {
		margin: 0.3rem 0 0;
	}
	.shell-result {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-top: 0.4rem;
	}
	.shell-exit {
		font-family: ui-monospace, monospace;
		font-size: 0.72rem;
		padding: 0.05rem 0.4rem;
		border-radius: 999px;
	}
	.exit-ok {
		background: #1f3a28;
		color: #6fd08c;
	}
	.exit-err {
		background: #3a1f1f;
		color: var(--error);
	}
	.shell-out {
		margin: 0.3rem 0 0;
	}
	.shell-err {
		margin: 0.2rem 0 0;
		color: #f28b82;
	}
	/* shell syntax highlighting */
	:global(.sh-cmd)     { color: #82cfff; }
	:global(.sh-flag)    { color: #be95ff; }
	:global(.sh-str)     { color: #42be65; }
	:global(.sh-var)     { color: #ff832b; }
	:global(.sh-op)      { color: #ee5396; }
	:global(.sh-comment) { color: var(--muted); font-style: italic; }
</style>
