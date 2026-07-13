<script>
	/** @type {{ text: string }} */
	let { text } = $props();
</script>

<div class="flex items-start gap-3">
	<span class="mt-0.5 font-mono text-sm font-bold text-primary select-none" aria-hidden="true">
		&gt;
	</span>
	<p class="min-w-0 flex-1 font-mono text-sm leading-relaxed whitespace-pre-wrap text-foreground">
		{text}
	</p>
</div>
