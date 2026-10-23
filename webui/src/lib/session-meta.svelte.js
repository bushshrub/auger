// Sidebar-facing state per session: a title (the user's local rename, else the
// first message) and whether the session is busy or waiting for approval. The
// active SessionView writes its own entry; the sidebar reads all of them.

const KEY = 'auger.titles';

/** @returns {Record<string, string>} */
function loadTitles() {
	try {
		return JSON.parse(localStorage.getItem(KEY) ?? '{}');
	} catch {
		return {};
	}
}

/** @type {Record<string, string>} Local renames, keyed by session id. */
export const titles = $state(loadTitles());

/** @type {Record<string, { firstMessage?: string, busy?: boolean, approval?: boolean }>} */
export const sessionMeta = $state({});

/**
 * @param {string} id
 * @param {string} title
 */
export function renameSession(id, title) {
	const clean = title.trim();
	if (clean) titles[id] = clean;
	else delete titles[id];
	try {
		localStorage.setItem(KEY, JSON.stringify(titles));
	} catch {
		// Storage may be blocked; the rename still lasts for this page load.
	}
}

/**
 * @param {string} id
 * @returns {string}
 */
export function sessionTitle(id) {
	return titles[id] || sessionMeta[id]?.firstMessage || `Session ${id.slice(0, 8)}`;
}
