<template>
  <nav class="rail" aria-label="Launcher">
    <button
      v-for="item in top"
      :key="item.id"
      type="button"
      :class="{ on: current === item.id, dot: item.dot }"
      :aria-label="item.label"
      :aria-current="current === item.id ? 'page' : null"
      :title="item.label"
      @click="$emit('go', item.id)"
    >
      <svg viewBox="0 0 24 24" v-html="item.icon"></svg>
    </button>
    <span class="grow"></span>
    <button
      v-for="item in bottom"
      :key="item.id"
      type="button"
      :class="{ on: current === item.id }"
      :aria-label="item.label"
      :aria-current="current === item.id ? 'page' : null"
      :title="item.label"
      @click="$emit('go', item.id)"
    >
      <svg viewBox="0 0 24 24" v-html="item.icon"></svg>
    </button>
    <span class="version">{{ release.split('-')[0] }}</span>
  </nav>
</template>

<script setup>
// The side menu: the screens as icons (named on hover), Support and Settings at the foot. Chat,
// Support and the Roadmap are the community network's, shown to its players only.
import { computed } from 'vue';

const props = defineProps({ current: String, release: { type: String, default: '' }, community: Boolean, unread: { type: Number, default: 0 } });
defineEmits(['go']);

const ICONS = {
  play: '<path d="M7 4.5v15l12-7.5z"/>',
  servers: '<rect x="4" y="4" width="16" height="6" rx="2"/><rect x="4" y="14" width="16" height="6" rx="2"/>',
  chat: '<path d="M4 5h16v11H9l-5 4z"/>',
  intel: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M8 9h8M8 13h8M8 17h5"/>',
  support: '<circle cx="12" cy="12" r="8"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6M12 17h.01"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1"/>',
};
const top = computed(() => [
  { id: 'play', label: 'Play', icon: ICONS.play },
  { id: 'servers', label: 'Servers', icon: ICONS.servers },
  ...(props.community ? [{ id: 'chat', label: 'Chat', icon: ICONS.chat, dot: props.unread > 0 }] : []),
  { id: 'intel', label: 'Intel: news and the roadmap', icon: ICONS.intel },
]);
const bottom = computed(() => [
  ...(props.community ? [{ id: 'support', label: 'Support', icon: ICONS.support }] : []),
  { id: 'settings', label: 'Settings', icon: ICONS.settings },
]);
</script>

<style scoped>
.rail { width: 60px; flex: none; display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 16px 0 12px; background: var(--rail); border-right: 1px solid var(--line); }
button { position: relative; width: 42px; height: 42px; border-radius: 8px; display: grid; place-items: center; color: var(--muted); transition: background .2s, color .2s; }
button svg { width: 20px; height: 20px; }
button:hover { background: var(--control); color: var(--fg); }
button.on { background: var(--selected); color: var(--accent); }
button.on::before { content: ""; position: absolute; left: 0; top: 8px; bottom: 8px; width: 2px; border-radius: 1px; background: var(--accent); animation: grow .35s var(--ease); }
button.dot::after { content: ""; position: absolute; top: 7px; right: 7px; width: 7px; height: 7px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 2px var(--rail); animation: breathe 2.4s ease-in-out infinite; }
.grow { flex: 1; }
.version { font: 10px PlexMono, monospace; color: var(--muted); margin-top: 6px; }
@keyframes grow { from { transform: scaleY(0); } }
@keyframes breathe { 50% { opacity: .45; } }
</style>
