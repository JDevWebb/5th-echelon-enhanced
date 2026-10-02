<template>
  <PageTop title="Network" sub="Bandwidth, relayed traffic and round trips.">
    <RangePicker v-model="range" :options="RANGES.slice(0, 5)" />
  </PageTop>
  <div v-if="data" class="grid cols-2">
    <section v-for="c in charts" :key="c.title" class="panel">
      <header><h2>{{ c.title }}</h2><div class="legend"><span v-for="s in c.series" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
      <LineChart :series="c.series" :from="data.from" :to="data.to" :format="c.format" />
    </section>
  </div>
  <div v-else class="empty">Loading…</div>
  <section class="panel">
    <header><h2>Players' ping to each server</h2><span class="small muted">As their launchers measured it, by country · last {{ range > 86400 ? RANGES.find(r => r[0] === range)?.[1] : '24 h' }}</span></header>
    <div v-if="byServer.length" class="grid cols-2">
      <div v-for="[sid, rows] in byServer" :key="sid">
        <h3 style="margin-bottom: 6px">{{ names.get(sid) || sid }}</h3>
        <table>
          <thead><tr><th>Country</th><th class="r">Median</th><th class="r">90th %</th><th class="r">Reports</th></tr></thead>
          <tbody>
            <tr v-for="(p, i) in rows" :key="i"><td>{{ p.country ? `${flag(p.country)} ${p.country}` : 'Unknown' }}</td><td class="r">{{ fmt.ms(p.median) }}</td><td class="r">{{ fmt.ms(p.p90) }}</td><td class="r">{{ fmt.n(p.reports) }}</td></tr>
          </tbody>
        </table>
      </div>
    </div>
    <div v-else class="empty">No launcher has reported pings yet.</div>
  </section>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import LineChart from '../components/LineChart.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { colorFor, flag, fmt, RANGES, serverName, seriesFor } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const range = ref(86400);
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
const { data } = useLoad(() => api('GET', `/series?range=${range.value}`), () => range.value);
const { data: pings } = useLoad(() => api('GET', `/pings?range=${Math.max(range.value, 86400)}`), () => range.value, { refresh: false });
const charts = computed(() => {
  const d = data.value;
  const pingLines = Object.keys(d.pings || {}).sort().map(id => ({ name: names.value.get(id) || id, color: colorFor(id), points: d.pings[id].map(p => ({ t: p.t, v: p.ms })) }));
  return [
    { title: 'Traffic out', series: seriesFor(d, 'tx', servers.value), format: fmt.rate },
    { title: 'Traffic in', series: seriesFor(d, 'rx', servers.value), format: fmt.rate },
    { title: 'Relayed between players', series: seriesFor(d, 'relayed', servers.value), format: fmt.rate },
    { title: 'Ping from the coordinator', series: pingLines, format: fmt.ms },
  ];
});
const byServer = computed(() => {
  const m = new Map();
  for (const p of pings.value?.pings || []) { if (!m.has(p.server)) m.set(p.server, []); m.get(p.server).push(p); }
  return [...m.entries()].map(([sid, rows]) => [sid, [...rows].sort((a, b) => b.reports - a.reports)]);
});
</script>
