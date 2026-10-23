<script>
	/** @type {{ text: string }} */
	let { text } = $props();
</script>

<div class="flex justify-end">
	<div
		class="max-w-[85%] rounded-2xl bg-muted px-4 py-2.5 text-[0.9375rem] leading-relaxed break-words whitespace-pre-wrap text-foreground"
	>
		{text}
	</div>
</div>
