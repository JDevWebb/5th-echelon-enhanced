<template>
  <header class="titlebar" data-tauri-drag-region @dblclick.self="win.toggleMaximize()">
    <span class="lights" aria-hidden="true"><i></i><i></i><i></i></span>
    <span class="product" data-tauri-drag-region>{{ product.toUpperCase() }} {{ release.toUpperCase() }}</span>
    <span class="spacer" data-tauri-drag-region @dblclick="win.toggleMaximize()"></span>
    <button class="wc" type="button" aria-label="Minimise" title="Minimise" @click="win.minimize()"><svg viewBox="0 0 12 12"><path d="M1 6.5h10" /></svg></button>
    <button class="wc" type="button" aria-label="Maximise" title="Maximise" @click="win.toggleMaximize()"><svg viewBox="0 0 12 12"><rect x="1.5" y="1.5" width="9" height="9" /></svg></button>
    <button class="wc close" type="button" aria-label="Close" title="Close" @click="win.close()"><svg viewBox="0 0 12 12"><path d="M1.5 1.5l9 9M10.5 1.5l-9 9" /></svg></button>
  </header>
</template>

<script setup>
// The window's own title bar: dragged to move it, double-clicked to maximise it, its buttons the
// window's (the system's frame is off, tauri.conf.json).
import { win } from '../lib/tauri.js';

defineProps({ product: { type: String, default: '' }, release: { type: String, default: '' } });
</script>

<style scoped>
.titlebar { height: 34px; flex: none; display: flex; align-items: center; background: var(--rail); border-bottom: 1px solid var(--line); }
.lights { display: flex; gap: 4px; padding: 0 12px 0 14px; }
.lights i { width: 5px; height: 5px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 8px var(--accent); animation: breathe 3.2s ease-in-out infinite; }
.lights i:nth-child(2) { animation-delay: .25s; }
.lights i:nth-child(3) { animation-delay: .5s; }
.product { font: 11px PlexMono, monospace; letter-spacing: .12em; color: var(--muted); }
.spacer { flex: 1; height: 100%; }
.wc { width: 46px; height: 34px; display: grid; place-items: center; color: var(--soft); transition: background .15s, color .15s; }
.wc svg { width: 11px; height: 11px; stroke-width: 1.2; }
.wc:hover { background: var(--control-hover); color: var(--fg); }
.wc.close:hover { background: var(--bad); color: #fff; }
@keyframes breathe { 50% { opacity: .45; box-shadow: 0 0 2px var(--accent); } }
</style>
