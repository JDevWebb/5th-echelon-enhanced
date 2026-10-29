<template>
  <Transition name="bar">
    <div v-if="a" class="activity" :class="a.state" role="status" aria-live="polite">
      <span class="track" aria-hidden="true"><i :style="a.progress != null ? { width: `${a.progress * 100}%` } : null" :class="{ moving: a.state === 'working' && a.progress == null }"></i></span>
      <span class="mark" aria-hidden="true"></span>
      <span class="text">
        <b>{{ a.title }}</b>
        <span v-if="a.step" class="muted"> · {{ a.step }}</span>
        <span v-if="a.message" class="soft"> · {{ a.message }}</span>
      </span>
      <span v-if="a.waiting" class="caps">+{{ a.waiting }} more</span>
      <button v-if="a.state !== 'working'" type="button" class="close" aria-label="Dismiss" title="Dismiss" @click="dismiss">
        <svg viewBox="0 0 12 12"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" /></svg>
      </button>
    </div>
  </Transition>
</template>

<script setup>
// The activity bar along the bottom (the egui launcher's activity.rs): what the launcher is
// doing, what it just did, and what failed, one at a time.
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { call, listen } from '../lib/tauri.js';

const a = ref(null);
let stop = null;
let timer = null;

async function refresh() {
  a.value = await call('activity');
}

async function dismiss() {
  await call('dismiss', { id: a.value.id });
  refresh();
}

defineExpose({ refresh });

onMounted(async () => {
  stop = await listen('activity', refresh);
  // What's done goes after a few seconds: looked at again while something shows.
  timer = setInterval(() => a.value && refresh(), 1000);
  refresh();
});
onBeforeUnmount(() => {
  stop?.();
  clearInterval(timer);
});
</script>

<style scoped>
.activity { position: relative; flex: none; display: flex; align-items: center; gap: 12px; min-height: 44px; padding: 10px 24px 10px 32px; background: var(--sunken); border-top: 1px solid var(--control-line); }
.track { position: absolute; left: 0; right: 0; top: -1px; height: 2px; overflow: hidden; }
.track i { display: block; height: 100%; width: 0; background: var(--accent); transition: width .3s var(--ease); }
.track i.moving { width: 30%; animation: move 1.4s ease-in-out infinite; }
.done .track i { width: 100%; }
.failed .track i { width: 100%; background: var(--bad); }
.mark { width: 8px; height: 8px; border-radius: 50%; background: var(--accent); flex: none; }
.working .mark { animation: breathe 1.2s ease-in-out infinite; }
.failed .mark { background: var(--bad); }
.text { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.text b { font-weight: 600; }
.close { width: 28px; height: 28px; border-radius: 4px; display: grid; place-items: center; color: var(--soft); }
.close:hover { background: var(--control-hover); color: var(--fg); }
.close svg { width: 10px; height: 10px; stroke-width: 1.4; }
.bar-enter-active, .bar-leave-active { transition: opacity .2s, transform .25s var(--ease); }
.bar-enter-from, .bar-leave-to { opacity: 0; transform: translateY(8px); }
@keyframes move { from { transform: translateX(-100%); } to { transform: translateX(340%); } }
@keyframes breathe { 50% { opacity: .35; } }
</style>
