<template>
  <div v-if="!buckets.length" class="empty">No traffic in this period yet.</div>
  <div v-else class="chart">
    <svg :viewBox="`0 0 ${W} ${H}`" role="img" aria-label="Data in and out per period, with what went through the relay" @pointerleave="hover = null">
      <g v-for="i in 5" :key="'g' + i">
        <line class="grid-line" :x1="L" :x2="W - R" :y1="y(top * (i - 1) / 4)" :y2="y(top * (i - 1) / 4)" />
        <text class="axis" :x="L - 6" :y="y(top * (i - 1) / 4) + 3.5" text-anchor="end">{{ fmt.size(top * (i - 1) / 4) }}</text>
      </g>
      <g v-for="(b, i) in buckets" :key="b.t" @pointerenter="hover = { i, b }">
        <rect :x="x(i)" :y="y(b.rx)" :width="bw" :height="Math.max(0, y(0) - y(b.rx))" class="in" rx="2" />
        <rect :x="x(i)" :y="y(b.rx + b.tx)" :width="bw" :height="Math.max(0, y(b.rx) - y(b.rx + b.tx))" class="out" rx="2" />
        <rect :x="x(i) - gap / 2" :y="T" :width="bw + gap" :height="H - T - B" fill="transparent" />
      </g>
      <path :d="relayLine" fill="none" class="relay" stroke-width="2" vector-effect="non-scaling-stroke" />
      <text v-for="(lbl, i) in ticks" :key="'t' + i" class="axis" :x="lbl.x" :y="H - 5" text-anchor="middle">{{ lbl.text }}</text>
    </svg>
    <div v-if="hover" class="tip" :style="{ left: `${Math.min(80, (hover.i + 1) / buckets.length * 100)}%`, top: '8px' }">
      <div class="small muted">{{ label(hover.b.t) }}</div>
      <div class="k"><span><i class="in"></i> In</span><b class="num">{{ fmt.size(hover.b.rx) }}</b></div>
      <div class="k"><span><i class="out"></i> Out</span><b class="num">{{ fmt.size(hover.b.tx) }}</b></div>
      <div class="k"><span><i class="relay"></i> Relayed</span><b class="num">{{ fmt.size(hover.b.relayed) }}</b></div>
    </div>
  </div>
</template>

<script setup>
// Stacked bars of data in and out per period, with relayed data as a line over them.
import { computed, ref } from 'vue';
import { fmt } from '../lib/fmt.js';

const props = defineProps({ buckets: { type: Array, default: () => [] }, step: String });
const W = 720, H = 220, L = 56, R = 8, T = 8, B = 22;
const hover = ref(null);
const n = computed(() => props.buckets.length);
const slot = computed(() => (W - L - R) / Math.max(1, n.value));
const gap = computed(() => Math.min(8, slot.value * 0.3));
const bw = computed(() => slot.value - gap.value);
const top = computed(() => {
  const m = Math.max(1, ...props.buckets.map(b => b.rx + b.tx)) * 1.08;
  const q = m / 4, p = 10 ** Math.floor(Math.log10(q));
  for (const k of [1, 2, 2.5, 5, 10]) if (q <= k * p) return 4 * k * p;
  return m;
});
const x = i => L + i * slot.value + gap.value / 2;
const y = v => T + (1 - Math.min(v, top.value) / top.value) * (H - T - B);
const relayLine = computed(() => props.buckets.map((b, i) => `${i ? 'L' : 'M'}${(x(i) + bw.value / 2).toFixed(1)},${y(b.relayed).toFixed(1)}`).join(''));
const label = t => {
  const d = new Date(t * 1000);
  if (props.step === 'month') return d.toLocaleDateString(undefined, { month: 'long', year: 'numeric', timeZone: 'UTC' });
  if (props.step === 'day') return d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', timeZone: 'UTC' });
  return d.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
};
const ticks = computed(() => {
  const every = Math.max(1, Math.ceil(n.value / 6));
  return props.buckets.map((b, i) => ({ i, b })).filter(({ i }) => i % every === 0).map(({ i, b }) => ({
    x: x(i) + bw.value / 2,
    text: props.step === 'month' ? new Date(b.t * 1000).toLocaleDateString(undefined, { month: 'short', timeZone: 'UTC' })
      : props.step === 'day' ? new Date(b.t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', timeZone: 'UTC' })
      : new Date(b.t * 1000).toLocaleString(undefined, props.step === 'hour' ? { hour: '2-digit', minute: '2-digit' } : { weekday: 'short', hour: '2-digit' }),
  }));
});
</script>

<style scoped>
.in { fill: var(--info); }
.out { fill: var(--accent); }
.relay { stroke: var(--warn); }
.tip i.in { background: var(--info); }
.tip i.out { background: var(--accent); }
.tip i.relay { background: var(--warn); }
</style>
