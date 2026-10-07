import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  // Mode dev: panggilan relatif /api/... diteruskan ke backend, sama seperti satu origin di produksi.
  server: { proxy: { '/api': 'http://127.0.0.1:7410' } }
});

