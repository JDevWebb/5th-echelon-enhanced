<template>
  <div class="table-wrap">
    <div class="heat" role="img" aria-label="Average players online by day of the week and hour, in your time zone">
      <template v-for="(row, d) in grid" :key="d">
        <span class="day">{{ DAYS[d] }}</span>
        <span v-for="(v, h) in row" :key="h" class="cell" :style="{ background: color(v) }" :title="`${DAYS[d]} ${String(h).padStart(2, '0')}:00 · ${v == null ? 'no data' : fmt.n(v, 1) + ' players on average'}`"></span>
      </template>
      <span></span>
      <span v-for="h in 24" :key="'h' + h" class="hour">{{ (h - 1) % 6 === 0 ? String(h - 1).padStart(2, '0') : '' }}</span>
    </div>
  </div>
</template>

<script setup>
// Average players online by weekday and hour of the day, in the viewer's own time zone.
import { computed } from 'vue';
import { fmt } from '../lib/fmt.js';

const props = defineProps({ hours: { type: Array, default: () => [] } });
const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
const grid = computed(() => {
  const sum = Array.from({ length: 7 }, () => Array(24).fill(0));
  const n = Array.from({ length: 7 }, () => Array(24).fill(0));
  for (const h of props.hours) {
    const d = new Date(h.t * 1000);
    const day = (d.getDay() + 6) % 7, hour = d.getHours();
    sum[day][hour] += h.avg;
    n[day][hour]++;
  }
  return sum.map((row, d) => row.map((v, h) => (n[d][h] ? v / n[d][h] : null)));
});
const max = computed(() => Math.max(0.01, ...grid.value.flat().filter(v => v != null)));
const color = v => (v == null ? 'transparent' : `color-mix(in srgb, var(--accent) ${Math.round(8 + (v / max.value) * 92)}%, var(--panel-2))`);
</script>

<style scoped>
.heat { display: grid; grid-template-columns: 40px repeat(24, minmax(14px, 1fr)); gap: 3px; min-width: 560px; align-items: center; }
.day, .hour { font-size: 11px; color: var(--muted); }
.hour { font-family: var(--mono); }
.cell { height: 20px; border-radius: 4px; border: 1px solid var(--line); }
</style>
