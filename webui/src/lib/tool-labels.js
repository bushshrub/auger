// One-line labels for tool calls, shown in the transcript rows.

/**
 * @param {string} raw
 * @returns {Record<string, unknown>}
 */
export function parseArgs(raw) {
	try {
		const parsed = JSON.parse(raw);
		return typeof parsed === 'object' && parsed !== null ? parsed : {};
	} catch {
		return {};
	}
}

/**
 * The program a shell command runs, skipping leading `cd` segments.
 * @param {string} command
 * @returns {string | null}
 */
export function shellProgram(command) {
	const segments = command.split(/;|&&|\|\||\||\n/);
	for (const seg of segments) {
		const word = seg.trim().split(/\s+/)[0];
		if (word && word !== 'cd') return word;
	}
	return null;
}

/**
 * Splits an MCP tool name `mcp__<server>__<tool>`; null for other names.
 * @param {string} name
 * @returns {{ server: string, tool: string } | null}
 */
export function parseMcpName(name) {
	if (!name.startsWith('mcp__')) return null;
	const rest = name.slice(5);
	const at = rest.indexOf('__');
	if (at <= 0) return null;
	return { server: rest.slice(0, at), tool: rest.slice(at + 2) };
}

/**
 * @param {string} name
 * @param {string} rawArgs
 * @returns {string}
 */
export function toolLabel(name, rawArgs) {
	const args = parseArgs(rawArgs);
	const str = (/** @type {unknown} */ v) => (typeof v === 'string' ? v : '');
	if (name === 'shell') {
		const program = shellProgram(str(args.command));
		return program ? `Ran ${program}...` : 'Ran a command';
	}
	if (name === 'read_file') return `Read ${str(args.path)}`;
	if (name === 'edit_file') return `Edited ${str(args.path)}`;
	if (name === 'write_file') return `Wrote ${str(args.path)}`;
	if (name === 'grep') return `Searched "${str(args.pattern)}"`;
	if (name === 'glob') return `Matched ${str(args.pattern)}`;
	if (name === 'list_files') return `Listed ${str(args.path) || '.'}`;
	if (name === 'todo_list') return 'Updated todos';
	if (name === 'web_search') return `Searched the web for "${str(args.query)}"`;
	if (name === 'web_fetch' || name === 'web_fetch_text' || name === 'fetch_content')
		return `Fetched ${str(args.url) || str(args.query)}`;
	const mcp = parseMcpName(name);
	if (mcp) return `Called ${mcp.server}.${mcp.tool}`;
	return name;
}

/**
 * Collapsed summary of a group: "Ran N commands" when every call is shell.
 * @param {{ name: string }[]} calls
 * @returns {string}
 */
export function groupLabel(calls) {
	const n = calls.length;
	if (calls.every((c) => c.name === 'shell')) return `Ran ${n} command${n === 1 ? '' : 's'}`;
	return `Used ${n} tool${n === 1 ? '' : 's'}`;
}
