<script>
	import {
		Check,
		ChevronDown,
		ChevronRight,
		CircleAlert,
		Download,
		FilePen,
		FilePlus,
		FileText,
		FolderTree,
		Globe,
		ListTodo,
		LoaderCircle,
		Search,
		ShieldCheck,
		Terminal,
		X
	} from '@lucide/svelte';
	import DiffViewer from '$lib/DiffViewer.svelte';

	/**
	 * @type {{
	 *   call: import('$lib/session.svelte.js').UiToolCall,
	 *   sessionArchived?: boolean,
	 *   onRespond: (toolCallId: string, approved: boolean, message?: string) => Promise<void>
	 * }}
	 */
	let { call, sessionArchived = false, onRespond } = $props();

	/** @type {Record<string, typeof Terminal>} */
	const TOOL_ICONS = {
		shell: Terminal,
		edit_file: FilePen,
		write_file: FilePlus,
		read_file: FileText,
		grep: Search,
		glob: FolderTree,
		list_files: FolderTree,
		todo_list: ListTodo,
		web_search: Globe,
		web_fetch: Download,
		web_fetch_text: Download,
		fetch_content: Download
	};

	/**
	 * @param {string} raw
	 * @returns {Record<string, unknown>}
	 */
	function parseArgs(raw) {
		try {
			const parsed = JSON.parse(raw);
			return typeof parsed === 'object' && parsed !== null ? parsed : {};
		} catch {
			return {};
		}
	}

	/**
	 * @param {string} name
	 * @param {Record<string, unknown>} args
	 * @returns {string}
	 */
	function argSummary(name, args) {
		if (name === 'shell') return String(args.command ?? '');
		if (name === 'edit_file' || name === 'write_file' || name === 'read_file')
			return String(args.path ?? '');
		if (name === 'grep') return `"${String(args.pattern ?? '')}" in ${String(args.path ?? '.')}`;
		if (name === 'glob') return String(args.pattern ?? '');
		if (name === 'list_files') return String(args.path ?? '.');
		if (name === 'web_search') return String(args.query ?? '');
		if (name === 'web_fetch' || name === 'web_fetch_text' || name === 'fetch_content')
			return String(args.url ?? args.query ?? '');
		const first = Object.values(args)[0];
		return first !== undefined ? String(first) : '';
	}

	let open = $state(false);
	let responding = $state(false);

	const args = $derived(parseArgs(call.arguments));
	const Icon = $derived(TOOL_ICONS[call.name] ?? Terminal);
	const summary = $derived(argSummary(call.name, args));
	const isPending = $derived(call.status === 'pending_approval');
	const isEdit = $derived(call.name === 'edit_file' && 'old_string' in args && 'new_string' in args);

	// Auto-open the card whenever it needs a decision.
	$effect(() => {
		if (isPending) open = true;
	});

	/** @param {boolean} approved */
	async function respond(approved) {
		responding = true;
		try {
			await onRespond(call.id, approved);
		} catch {
			// Failure is surfaced through the session error banner.
		} finally {
			responding = false;
		}
	}
</script>

<div
	class={`overflow-hidden rounded-lg border bg-card ${isPending ? 'border-primary/50' : 'border-border'}`}
>
	<button
		type="button"
		onclick={() => (open = !open)}
		class="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-accent/40"
		aria-expanded={open}
	>
		{#if open}
			<ChevronDown class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
		{:else}
			<ChevronRight class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
		{/if}
		<Icon class="size-3.5 shrink-0 text-primary" aria-hidden="true" />
		<span class="font-mono text-xs font-semibold text-foreground">{call.name}</span>
		<span class="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{summary}</span>

		{#if call.status === 'pending_approval'}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-primary/15 px-2 py-0.5 font-mono text-[10px] font-medium text-primary"
			>
				<CircleAlert class="size-3" aria-hidden="true" />
				needs approval
			</span>
		{:else if call.status === 'running'}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-muted px-2 py-0.5 font-mono text-[10px] text-muted-foreground"
			>
				<LoaderCircle class="size-3 animate-spin" aria-hidden="true" />
				running
			</span>
		{:else if call.status === 'done'}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-success/15 px-2 py-0.5 font-mono text-[10px] text-success"
			>
				{#if call.autoApproved}
					<ShieldCheck class="size-3" aria-hidden="true" />
					auto
				{:else}
					<Check class="size-3" aria-hidden="true" />
					done
				{/if}
			</span>
		{:else if call.status === 'denied'}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 font-mono text-[10px] text-destructive"
			>
				<X class="size-3" aria-hidden="true" />
				denied
			</span>
		{:else}
			<span
				class="flex shrink-0 items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 font-mono text-[10px] text-destructive"
			>
				<CircleAlert class="size-3" aria-hidden="true" />
				error
			</span>
		{/if}
	</button>

	{#if open}
		<div class="flex flex-col gap-2 border-t border-border px-3 py-2.5">
			{#if isEdit}
				<DiffViewer
					oldContent={String(args.old_string ?? '')}
					newContent={String(args.new_string ?? '')}
					fileName={String(args.path ?? '')}
				/>
			{:else}
				<pre
					class="overflow-x-auto rounded-md bg-sidebar p-2.5 font-mono text-xs text-foreground/80 auger-scroll">{JSON.stringify(
						args,
						null,
						2
					)}</pre>
			{/if}

			{#if call.result !== undefined}
				<div>
					<p class="mb-1 font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
						output
					</p>
					<pre
						class="max-h-64 overflow-auto rounded-md bg-sidebar p-2.5 font-mono text-xs leading-relaxed whitespace-pre-wrap text-foreground/80 auger-scroll">{call.result}</pre>
				</div>
			{/if}

			{#if call.error !== undefined}
				<pre
					class="overflow-x-auto rounded-md bg-destructive/10 p-2.5 font-mono text-xs text-destructive auger-scroll">{call.error}</pre>
			{/if}

			{#if isPending && !sessionArchived}
				<div class="flex flex-wrap items-center gap-2 pt-1">
					<p class="mr-auto text-xs text-muted-foreground">Approve this tool call?</p>
					<button
						type="button"
						disabled={responding}
						onclick={() => respond(false)}
						class="flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs font-medium text-foreground hover:bg-accent disabled:opacity-50"
					>
						<X class="size-3.5" aria-hidden="true" />
						Deny
					</button>
					<button
						type="button"
						disabled={responding}
						onclick={() => respond(true)}
						class="flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
					>
						<Check class="size-3.5" aria-hidden="true" />
						Approve
					</button>
				</div>
			{/if}
		</div>
	{/if}
</div>
