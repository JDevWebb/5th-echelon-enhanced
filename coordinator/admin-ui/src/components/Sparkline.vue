<template>
  <svg class="spark" viewBox="0 0 160 36" preserveAspectRatio="none" aria-hidden="true">
    <path v-if="area" :d="area" :style="{ fill: color, opacity: 0.15 }" />
    <path v-if="line" :d="line" fill="none" :style="{ stroke: color }" stroke-width="1.6" vector-effect="non-scaling-stroke" />
  </svg>
</template>

<script setup>
import { computed } from 'vue';
const props = defineProps({ values: { type: Array, default: () => [] }, color: { type: String, default: 'var(--accent)' } });
const coords = computed(() => {
  const v = props.values;
  if (v.length < 2) return null;
  const max = Math.max(...v) * 1.15 || 1;
  return v.map((y, i) => [(i / (v.length - 1)) * 160, 36 - (y / max) * 34]);
});
const line = computed(() => coords.value?.map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(1)} ${y.toFixed(1)}`).join(''));
const area = computed(() => coords.value && `${line.value}L160 36L0 36Z`);
</script>

<style scoped>
.spark { width: 100%; height: 36px; display: block; margin-top: 4px; }
</style>
