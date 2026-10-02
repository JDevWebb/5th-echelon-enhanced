<template>
  <div v-if="!hasData" class="empty">No data for this period yet.</div>
  <div v-else class="chart">
    <svg ref="svg" :viewBox="`0 0 ${W} ${height}`" role="img" :aria-label="series.map(s => s.name).join(', ')" @pointermove="move" @pointerleave="hover = null">
      <g v-for="i in 5" :key="'g' + i">
        <line class="grid-line" :x1="L" :x2="W - R" :y1="y(top * (i - 1) / 4)" :y2="y(top * (i - 1) / 4)" />
        <text class="axis" :x="L - 6" :y="y(top * (i - 1) / 4) + 3.5" text-anchor="end">{{ format(top * (i - 1) / 4) }}</text>
      </g>
      <text v-for="i in 5" :key="'t' + i" class="axis" :x="x(from + span * (i - 1) / 4)" :y="height - 5" :text-anchor="i === 1 ? 'start' : i === 5 ? 'end' : 'middle'">{{ timeLabel(from + span * (i - 1) / 4) }}</text>
      <path v-for="(d, i) in paths" :key="'p' + i" :d="d" fill="none" :style="{ stroke: series[i].color }" stroke-width="1.8" stroke-linejoin="round" vector-effect="non-scaling-stroke" />
      <line v-if="hover" class="hover-line" :x1="x(hover.t)" :x2="x(hover.t)" :y1="T" :y2="height - B" />
    </svg>
    <div v-if="hover" class="tip" :style="{ left: `${hover.left}px`, top: '8px' }">
      <div class="small muted">{{ new Date(hover.t * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) }}</div>
      <div v-for="s in series" :key="s.name" class="k">
        <span><i :style="{ background: s.color }"></i> {{ s.name }}</span>
        <b class="num">{{ valueAt(s, hover.t) }}</b>
      </div>
    </div>
  </div>
</template>

<script setup>
// A line chart: series [{ name, color, points: [{ t, v }] }] over [from, to] (seconds).
import { computed, ref } from 'vue';
import { fmt } from '../lib/fmt.js';

const props = defineProps({
  series: { type: Array, required: true },
  from: Number,
  to: Number,
  format: { type: Function, default: v => fmt.n(v) },
  height: { type: Number, default: 180 },
  max: Number,
});
const W = 720, L = 46, R = 8, T = 8, B = 22;
const svg = ref(null);
const hover = ref(null);

const all = computed(() => props.series.flatMap(s => s.points.filter(p => p.v != null).map(p => p.v)));
const hasData = computed(() => all.value.length > 0);
/** A top for the y axis whose quarters are round numbers. */
function niceMax(v) {
  if (!(v > 0)) return 4;
  const quarter = v / 4;
  const p = 10 ** Math.floor(Math.log10(quarter));
  for (const m of [1, 2, 2.5, 5, 10]) if (quarter <= m * p) return 4 * m * p;
  return 40 * p;
}
const top = computed(() => props.max ?? niceMax(Math.max(...all.value) * 1.05));
const span = computed(() => props.to - props.from);
const x = t => L + (t - props.from) / Math.max(1, span.value) * (W - L - R);
const y = v => T + (1 - Math.min(v, top.value) / top.value) * (props.height - T - B);
const timeLabel = t => new Date(t * 1000).toLocaleString(undefined, span.value > 3 * 86400 ? { month: 'short', day: 'numeric' } : { hour: '2-digit', minute: '2-digit' });

const paths = computed(() => {
  const step = props.series.reduce((m, s) => {
    for (let i = 1; i < s.points.length; i++) m = Math.min(m, s.points[i].t - s.points[i - 1].t);
    return m;
  }, Infinity);
  return props.series.map(s => {
    let d = '', prev = null;
    for (const p of s.points) {
      if (p.v == null) { prev = null; continue; }
      // A gap of more than a few steps breaks the line (a server that was down).
      const jump = !prev || (Number.isFinite(step) && p.t - prev.t > step * 3.5);
      d += `${jump ? 'M' : 'L'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`;
      prev = p;
    }
    return d;
  });
});

const times = computed(() => [...new Set(props.series.flatMap(s => s.points.map(p => p.t)))].sort((a, b) => a - b));
function move(ev) {
  const rect = svg.value.getBoundingClientRect();
  const px = (ev.clientX - rect.left) / rect.width * W;
  const t = props.from + (px - L) / (W - L - R) * span.value;
  let best = times.value[0];
  for (const tt of times.value) if (Math.abs(tt - t) < Math.abs(best - t)) best = tt;
  const left = (x(best) / W) * rect.width;
  hover.value = { t: best, left: Math.min(Math.max(left + 12, 0), rect.width - 170) };
}
function valueAt(s, t) {
  const p = s.points.find(q => q.t === t);
  return p?.v == null ? '–' : props.format(p.v);
}
</script>
