import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Proxy /v1 to the agent-server so the browser avoids CORS in dev.
// Override target with AGENT_SERVER_URL env var.
const target = process.env.AGENT_SERVER_URL || 'http://127.0.0.1:3000';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		proxy: {
			'/v1': {
				target,
				changeOrigin: true
			}
		}
	}
});
