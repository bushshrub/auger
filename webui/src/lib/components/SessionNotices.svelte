<script>
	import NoticeBanner from './NoticeBanner.svelte';

	/** @type {{ error: string | null, toolsMissing: string[] }} */
	let { error, toolsMissing } = $props();

	// Dismissed notices stay hidden until their text changes.
	let dismissedError = $state(/** @type {string | null} */ (null));
	let dismissedTools = $state(/** @type {string | null} */ (null));

	const toolsText = $derived(toolsMissing.length ? `Tools no longer available: ${toolsMissing.join(', ')}` : null);
</script>

{#if error && error !== dismissedError}
	<NoticeBanner message={error} tone="error" ondismiss={() => (dismissedError = error)} />
{/if}
{#if toolsText && toolsText !== dismissedTools}
	<NoticeBanner message={toolsText} ondismiss={() => (dismissedTools = toolsText)} />
{/if}
