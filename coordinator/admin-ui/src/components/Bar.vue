<template>
  <div class="bar">
    <span class="muted">{{ label }}</span>
    <div class="track"><div class="fill" :class="cls" :style="{ width: `${(f * 100).toFixed(1)}%` }"></div></div>
    <span class="num" style="text-align: right">{{ text }}</span>
  </div>
</template>

<script setup>
import { computed } from 'vue';
const props = defineProps({ label: String, frac: Number, text: String });
const f = computed(() => Math.max(0, Math.min(1, props.frac || 0)));
const cls = computed(() => (f.value > 0.9 ? 'bad' : f.value > 0.75 ? 'warn' : ''));
</script>
