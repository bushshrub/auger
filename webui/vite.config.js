import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// When AGENT_SERVER_URL is set, proxy /v1 to the real agent-server.
// Without it, SvelteKit server routes under /v1 serve a built-in mock.
const agentServerUrl = process.env.AGENT_SERVER_URL;

export default defineConfig({
	plugins: [sveltekit()],
	server: agentServerUrl
		? {
				proxy: {
					'/v1': { target: agentServerUrl, changeOrigin: true }
				}
		  }
		: {}
});
