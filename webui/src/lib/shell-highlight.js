// Tiny shell tokenizer for command display. Not a parser: it colours the
// command word after each operator, flags, quoted strings and variables.

/**
 * @typedef {'cmd' | 'flag' | 'str' | 'var' | 'op' | 'arg' | 'ws'} ShellKind
 * @typedef {{ text: string, kind: ShellKind }} ShellToken
 */

const TOKEN = /("(?:[^"\\]|\\.)*"|'[^']*')|(\$\{[^}]*\}|\$[\w]+)|(-{1,2}[\w][\w-]*)|(\|\||&&|[|;&<>])|(\s+)|(\S+)/g;

/**
 * @param {string} src
 * @returns {ShellToken[]}
 */
export function tokenizeShell(src) {
	/** @type {ShellToken[]} */
	const out = [];
	let atCommand = true;
	for (const m of src.matchAll(TOKEN)) {
		const [text, str, variable, flag, op, ws] = m;
		if (ws !== undefined) {
			out.push({ text, kind: 'ws' });
		} else if (op !== undefined) {
			out.push({ text, kind: 'op' });
			atCommand = true;
		} else if (str !== undefined) {
			out.push({ text, kind: 'str' });
			atCommand = false;
		} else if (variable !== undefined) {
			out.push({ text, kind: 'var' });
			atCommand = false;
		} else if (flag !== undefined) {
			out.push({ text, kind: 'flag' });
			atCommand = false;
		} else if (atCommand) {
			out.push({ text, kind: 'cmd' });
			atCommand = false;
		} else {
			out.push({ text, kind: 'arg' });
		}
	}
	return out;
}
