<script>
	import { tokenizeShell } from '$lib/shell-highlight.js';
	import CopyButton from './CopyButton.svelte';

	/** @type {{ command: string }} */
	let { command } = $props();

	const tokens = $derived(tokenizeShell(command));

	/** @type {Record<string, string>} */
	const KIND_CLASS = {
		cmd: 'text-success font-semibold',
		flag: 'text-warning',
		str: 'text-warning',
		var: 'text-primary',
		op: 'text-muted-foreground',
		arg: 'text-foreground',
		ws: ''
	};
</script>

<div class="relative rounded-md border border-border bg-background py-2 pr-20 pl-3 font-mono text-xs leading-relaxed">
	<div class="absolute top-1.5 right-1.5">
		<CopyButton text={command} />
	</div>
	<div class="flex gap-2">
		<span class="shrink-0 text-muted-foreground select-none" aria-hidden="true">$</span>
		<code class="min-w-0 flex-1 break-words whitespace-pre-wrap">{#each tokens as t, i (i)}<span class={KIND_CLASS[t.kind]}>{t.text}</span>{/each}</code>
	</div>
</div>
