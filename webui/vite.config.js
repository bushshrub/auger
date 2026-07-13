import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

// The browser talks to agent-server through the /v1 dev proxy so everything
// stays same-origin (the real server has no /v1 prefix and no CORS headers).
// Point AGENT_SERVER_URL at a running agent-server; defaults to localhost:3000.
export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
	server: {
		proxy: {
			'/v1': {
				target: process.env.AGENT_SERVER_URL ?? 'http://127.0.0.1:3000',
				changeOrigin: true,
				rewrite: (path) => path.replace(/^\/v1/, '')
			}
		}
	}
});
