// Markdown + LaTeX rendering for agent output.
//
// markdown-it runs with `html: false`, so any raw HTML in the (untrusted) model
// output is escaped rather than executed — the rendered string is safe to drop
// in with {@html}. KaTeX math is produced by our own renderer, not the model.

import MarkdownIt from 'markdown-it';
import katexPlugin from '@vscode/markdown-it-katex';

// CJS interop: the plugin function may sit one `.default` deep under Vite/Node.
const plugin = katexPlugin?.default ?? katexPlugin;

const md = new MarkdownIt({
	html: false,
	linkify: true,
	breaks: true
});

md.use(plugin, { throwOnError: false, errorColor: '#ff6b6b' });

// Open links in a new tab.
/** @type {import('markdown-it/lib/renderer.mjs').RenderRule} */
const defaultLinkOpen =
	md.renderer.rules.link_open ||
	((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));

/** @type {import('markdown-it/lib/renderer.mjs').RenderRule} */
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
	tokens[idx].attrSet('target', '_blank');
	tokens[idx].attrSet('rel', 'noopener noreferrer');
	return defaultLinkOpen(tokens, idx, options, env, self);
};

/**
 * Render markdown (with $…$, $$…$$, \(…\), \[…\] math) to safe HTML.
 * @param {string | undefined} src
 * @returns {string}
 */
export function renderMarkdown(src) {
	return md.render(src ?? '');
}
