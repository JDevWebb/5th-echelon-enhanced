<template>
  <span class="live" :class="live.status === 'live' ? 'on' : live.status" role="status">
    <i></i>{{ text }}
  </span>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { live } from '../lib/live.js';

const now = ref(Date.now());
let timer;
onMounted(() => { timer = setInterval(() => { now.value = Date.now(); }, 1000); });
onUnmounted(() => clearInterval(timer));

const text = computed(() => {
  if (live.status === 'live') {
    const s = live.at ? Math.max(0, Math.round((now.value - live.at) / 1000)) : null;
    return s == null ? 'Live' : s < 5 ? 'Live · just updated' : `Live · updated ${s < 120 ? s + ' s' : Math.round(s / 60) + ' min'} ago`;
  }
  if (live.status === 'retrying') return 'Reconnecting…';
  if (live.status === 'connecting') return 'Connecting…';
  return 'Not live';
});
</script>
