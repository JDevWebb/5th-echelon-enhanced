import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

// The coordinator serves dist/ (embedded at build time, see coordinator/build.rs) under a
// strict content security policy: scripts, styles and fonts only from the page's own
// origin. So nothing is inlined: no data: fonts, no inline module-preload script.
export default defineConfig({
  plugins: [vue()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2020',
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
  // `npm run dev` against a coordinator on this machine (--admin-listen 127.0.0.1:8701).
  server: { proxy: { '/api': { target: 'http://127.0.0.1:8701', ws: true } } },
});
