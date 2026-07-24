<script>
	import { renderMarkdown } from '$lib/markdown.js';

	/** @type {{ text: string, streaming?: boolean }} */
	let { text, streaming = false } = $props();

	// Rendered HTML is held in state and produced OFF the synchronous mount path.
	// Parsing markdown (+KaTeX) is expensive; doing it inline for every message
	// when a large transcript loads parses the whole thing at once and freezes the
	// tab (which also blocks the fetch for a send). So:
	//   - while streaming, coalesce to ~8 renders/sec (avoids O(n^2) per-delta reparse);
	//   - when settled, render at idle so a big transcript fills in progressively
	//     while the main thread stays responsive.
	let html = $state('');
	/** @type {(() => void) | null} */
	let cancelScheduled = null;

	/** @param {boolean} streamingNow */
	function schedule(streamingNow) {
		// A render is already queued; it will read the latest `text` when it runs.
		if (cancelScheduled) return;
		const run = () => {
			cancelScheduled = null;
			html = renderMarkdown(text);
		};
		if (streamingNow) {
			const id = setTimeout(run, 120);
			cancelScheduled = () => clearTimeout(id);
		} else if (typeof requestIdleCallback !== 'undefined') {
			const id = requestIdleCallback(run, { timeout: 1000 });
			cancelScheduled = () => cancelIdleCallback(id);
		} else {
			const id = setTimeout(run, 0);
			cancelScheduled = () => clearTimeout(id);
		}
	}

	$effect(() => {
		void text; // register as a dependency
		schedule(streaming);
	});

	$effect(() => () => cancelScheduled?.());
</script>

<!-- renderMarkdown escapes raw HTML (markdown-it html:false), safe for {@html} -->
<div class="auger-md">{@html html}</div>
