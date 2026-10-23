<script>
	import DiffViewer from '$lib/DiffViewer.svelte';
	import { parseArgs } from '$lib/tool-labels.js';
	import ShellCommand from './ShellCommand.svelte';

	/** @type {{ call: import('$lib/session.svelte.js').UiToolCall }} */
	let { call } = $props();

	const args = $derived(parseArgs(call.arguments));
	const isShell = $derived(call.name === 'shell');
	const isEdit = $derived(call.name === 'edit_file' && 'old_string' in args && 'new_string' in args);
	const shell = $derived(isShell ? parseShellResult(call.result) : null);
	const output = $derived(
		shell
			? [shell.stdout, shell.stderr].filter((s) => s.length > 0).join('\n')
			: (call.result ?? '')
	);

	/**
	 * @param {string | undefined} raw
	 * @returns {{ stdout: string, stderr: string, exit_code: number } | null}
	 */
	function parseShellResult(raw) {
		if (!raw) return null;
		try {
			const parsed = JSON.parse(raw);
			if (typeof parsed === 'object' && parsed !== null && 'exit_code' in parsed) {
				return {
					stdout: String(parsed.stdout ?? ''),
					stderr: String(parsed.stderr ?? ''),
					exit_code: Number(parsed.exit_code ?? 0)
				};
			}
		} catch {}
		return null;
	}
</script>

<div class="flex flex-col gap-2.5 px-3 pt-1 pb-3">
	{#if isEdit}
		<DiffViewer
			oldContent={String(args.old_string ?? '')}
			newContent={String(args.new_string ?? '')}
			fileName={String(args.path ?? '')}
		/>
	{:else if isShell}
		<ShellCommand command={String(args.command ?? '')} />
	{:else}
		<pre
			class="max-h-72 overflow-auto rounded-md border border-border bg-background p-3 font-mono text-xs whitespace-pre-wrap text-muted-foreground auger-scroll">{JSON.stringify(args, null, 2)}</pre>
	{/if}

	{#if call.result !== undefined}
		<div class="overflow-hidden rounded-md border border-border">
			<pre
				class="max-h-72 overflow-auto p-3 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap text-muted-foreground auger-scroll">{output}</pre>
			{#if shell}
				<div
					class={`border-t border-border px-3 py-1 font-mono text-[11px] ${shell.exit_code === 0 ? 'text-muted-foreground' : 'text-destructive'}`}
				>
					exit {shell.exit_code}
				</div>
			{/if}
		</div>
	{/if}

	{#if call.error !== undefined}
		<pre
			class="max-h-72 overflow-auto rounded-md bg-destructive/10 p-3 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap text-destructive auger-scroll">{call.error}</pre>
	{/if}
</div>
