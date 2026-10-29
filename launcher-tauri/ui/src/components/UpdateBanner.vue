<template>
  <Transition name="bar">
    <div v-if="banner" class="update" role="status">
      <template v-if="banner.updating"><i class="spin"></i><span>Updating to {{ banner.version }}…</span></template>
      <template v-else>
        <b>Version {{ banner.version }} is available.</b>
        <button type="button" class="btn primary small" @click="now">Update now</button>
        <a href="#" @click.prevent="call('open_url', { url: banner.page })">What's new</a>
        <button type="button" class="btn small" @click="later">Later</button>
      </template>
    </div>
  </Transition>
</template>

<script setup>
// A newer release, offered across the top (the egui launcher's update banner): Update now
// replaces the launcher and restarts it; Later hides it until a server refuses this one.
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { call, listen } from '../lib/tauri.js';

const banner = ref(null);
let stop = null;

async function load() {
  banner.value = await call('update_banner');
}

async function now() {
  banner.value.updating = true;
  call('update_now');
}

async function later() {
  banner.value = null;
  await call('update_later');
}

onMounted(async () => {
  stop = await listen('update', load);
  load();
});
onBeforeUnmount(() => stop?.());
</script>

<style scoped>
.update { flex: none; display: flex; align-items: center; gap: 14px; padding: 8px 20px; background: rgba(155, 224, 90, .14); border-bottom: 1px solid rgba(155, 224, 90, .3); }
.update b { font-weight: 600; }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.bar-enter-active, .bar-leave-active { transition: opacity .2s; }
.bar-enter-from, .bar-leave-to { opacity: 0; }
</style>
