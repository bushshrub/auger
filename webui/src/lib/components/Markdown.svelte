<script>
	import { renderMarkdown } from '$lib/markdown.js';

	/** @type {{ text: string }} */
	let { text } = $props();
</script>

<!-- renderMarkdown escapes raw HTML (markdown-it html:false), safe for {@html} -->
<div class="auger-md">{@html renderMarkdown(text)}</div>
