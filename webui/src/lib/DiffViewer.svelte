<script>
	import { DiffView, DiffModeEnum } from '@git-diff-view/svelte';
	import { createPatch } from 'diff';
	import '@git-diff-view/svelte/styles/diff-view.css';

	/** @type {{ oldContent: string, newContent: string, fileName?: string, language?: string }} */
	let { oldContent, newContent, fileName = '', language = '' } = $props();

	const langFromPath = (/** @type {string} */ path) => {
		const ext = path.split('.').pop()?.toLowerCase() ?? '';
		const map = /** @type {Record<string, string>} */ ({
			rs: 'rust', js: 'javascript', ts: 'typescript', tsx: 'typescript',
			jsx: 'javascript', py: 'python', rb: 'ruby', go: 'go',
			java: 'java', cpp: 'cpp', c: 'c', cs: 'csharp', sh: 'bash',
			json: 'json', toml: 'toml', yaml: 'yaml', yml: 'yaml',
			md: 'markdown', html: 'html', css: 'css', svelte: 'svelte',
			sql: 'sql', kt: 'kotlin', swift: 'swift',
		});
		return map[ext] ?? ext;
	};

	const resolvedLang = $derived(language || langFromPath(fileName));
	const baseName = $derived(fileName.split('/').pop() || 'file');

	/**
	 * Compute unified diff hunks from two strings using the `diff` package.
	 * DiffParser.parse() is called per element and expects a complete file diff
	 * starting with --- / +++ headers, so we pass the whole diff as one string
	 * (dropping only the two-line "Index: …\n===…" preamble).
	 * @param {string} oldStr
	 * @param {string} newStr
	 * @param {string} name
	 * @returns {string[]}
	 */
	function computeHunks(oldStr, newStr, name) {
		if (oldStr === newStr) return [];
		const patch = createPatch(name, oldStr, newStr, '', '', { context: 3 });
		// createPatch emits: "Index: …\n===…\n--- …\n+++ …\n<hunks>"
		// Drop lines 0-1 ("Index:" and "===…"); keep "--- …" onward as one string.
		const body = patch.split('\n').slice(2).join('\n');
		return [body];
	}

	const hunks = $derived(computeHunks(oldContent, newContent, baseName));
</script>

<div class="diff-wrapper">
	<DiffView
		data={{
			oldFile: { fileName: baseName, fileLang: resolvedLang, content: oldContent },
			newFile: { fileName: baseName, fileLang: resolvedLang, content: newContent },
			hunks,
		}}
		diffViewTheme="dark"
		diffViewMode={DiffModeEnum.Unified}
		diffViewHighlight={true}
		diffViewFontSize={13}
	/>
</div>

<style>
	.diff-wrapper {
		margin-top: 0.4rem;
		border-radius: 6px;
		overflow: hidden;
		border: 1px solid var(--border);
		font-family: ui-monospace, monospace;
	}
</style>
