<template>
  <template v-if="o && !sv">
    <PageTop title="Server not found" />
    <p><RouterLink to="/servers">← All servers</RouterLink></p>
  </template>
  <template v-else-if="sv">
    <PageTop :title="serverName(sv)" :sub="[sv.listing?.region, sv.listing?.host, `id ${sv.id}`].filter(Boolean).join(' · ')">
      <RangePicker v-model="range" :options="RANGES" />
      <button class="small" type="button" @click="purge">Release unused names</button>
      <button class="danger small" type="button" @click="remove">Remove</button>
    </PageTop>
    <p><RouterLink class="small" to="/servers">← All servers</RouterLink></p>
    <div v-if="data" class="grid cols-2">
      <section v-for="c in charts" :key="c.title" class="panel">
        <header><h2>{{ c.title }}</h2><div class="legend"><span v-for="s in c.series" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart :series="c.series" :from="data.from" :to="data.to" :format="c.format" :max="c.max" />
      </section>
    </div>
    <div v-else class="empty">Loading…</div>
  </template>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import LineChart from '../components/LineChart.vue';
import RangePicker from '../components/RangePicker.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, PALETTE, RANGES, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ id: String });
const router = useRouter();
const range = ref(86400);
onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const sv = computed(() => o.value?.servers.find(x => x.id === props.id));
const { data } = useLoad(() => api('GET', `/series?range=${range.value}`), () => range.value);

const charts = computed(() => {
  const pts = data.value?.points?.[props.id] || [];
  const one = (name, field, color) => ({ name, color, points: pts.map(p => ({ t: p.t, v: p[field] })) });
  const pings = (data.value?.pings?.[props.id] || []).map(p => ({ t: p.t, v: p.ms }));
  const memTotal = sv.value?.metrics?.system?.mem_total;
  return [
    { title: 'Players', series: [one('Online', 'players', PALETTE[0]), one('In a match', 'in_match', PALETTE[1])] },
    { title: 'CPU', series: [one('Machine', 'cpu', PALETTE[0]), one('Server process', 'process_cpu', PALETTE[2])], format: v => fmt.pct(v), max: 100 },
    { title: 'Memory', series: [one('Used', 'mem_used', PALETTE[0]), one('Server process', 'rss', PALETTE[2])], format: fmt.bytes, max: memTotal || undefined },
    { title: 'Bandwidth', series: [one('In', 'rx', PALETTE[1]), one('Out', 'tx', PALETTE[0]), one('Relayed', 'relayed', PALETTE[2])], format: fmt.rate },
    { title: 'Sign-ins per minute', series: [one('Game', 'logins', PALETTE[0]), one('Failed', 'failed_logins', PALETTE[4]), one('New accounts', 'registrations', PALETTE[1])], format: v => fmt.n(v, 1) },
    { title: 'Ping from the coordinator', series: [{ name: 'Round trip', color: PALETTE[3], points: pings }], format: fmt.ms },
  ];
});

async function purge() {
  if (!await confirmBox(`Release ${serverName(sv.value)}'s unused names?`, 'Its links made over an hour ago for identities never seen online, and linked nowhere else, go with the names only they held. For a server reserving names with throwaway identities.', 'Release names', true)) return;
  try { toast((await api('POST', `/servers/${encodeURIComponent(props.id)}/purge-names`)).message); } catch (e) { toast(e.message, true); }
}
async function remove() {
  if (!await confirmBox(`Remove ${serverName(sv.value)}?`, 'Its links and the names only it used go. It can join again with the join token unless you make a new one.', 'Remove server', true)) return;
  try { toast((await api('DELETE', `/servers/${encodeURIComponent(props.id)}`)).message); router.push('/servers'); } catch (e) { toast(e.message, true); }
}
</script>
