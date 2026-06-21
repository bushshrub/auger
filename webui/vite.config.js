import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// SvelteKit server routes under src/routes/v1 handle proxying to AGENT_SERVER_URL
// with response-shape transformation. Direct vite proxy is not used because the
// real server has no /v1 prefix and the shapes differ.

export default defineConfig({
	plugins: [sveltekit()]
});
