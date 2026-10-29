import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

// Tauri serves dist/ from the binary under a strict content security policy (tauri.conf.json):
// scripts, styles and fonts only from the page's own origin, so nothing is inlined.
export default defineConfig({
  plugins: [vue()],
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2020',
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
});
