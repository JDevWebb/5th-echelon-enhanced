<template>
  <PageTop title="Bandwidth" :sub="data ? `${rangeLabel} · data in and out of every server, and what went through the relay` : 'Data in and out of every server.'">
    <RangePicker v-model="range" :options="REPORT_RANGES" />
    <select v-model="server" aria-label="Server" style="width: auto">
      <option value="">All servers</option>
      <option v-for="sv in servers" :key="sv.id" :value="sv.id">{{ serverName(sv) }}</option>
    </select>
    <button type="button" :disabled="!data" @click="exportCsv">Export CSV</button>
  </PageTop>
  <template v-if="data">
    <div class="kpis kpis-6">
      <div class="kpi"><span class="label">Transferred</span><span class="v">{{ fmt.size(total) }}</span><span class="d">in and out</span></div>
      <div class="kpi"><span class="label">In</span><span class="v" style="color: var(--info)">{{ fmt.size(data.totals.rx) }}</span><span class="d">players to the servers</span></div>
      <div class="kpi"><span class="label">Out</span><span class="v" style="color: var(--accent)">{{ fmt.size(data.totals.tx) }}</span><span class="d">servers to the players</span></div>
      <div class="kpi"><span class="label">Relayed</span><span class="v" style="color: var(--warn)">{{ fmt.size(data.totals.relayed) }}</span><span class="d">{{ total ? fmt.pct(data.totals.relayed / total * 100) : '–' }} of all traffic</span></div>
      <div class="kpi"><span class="label">Peak</span><span class="v">{{ fmt.rate(data.peak.bps) }}</span><span class="d">{{ data.peak.bps ? `${names.get(data.peak.server) || data.peak.server} · ${fmt.when(data.peak.t)}` : '–' }}</span></div>
      <div class="kpi"><span class="label">95th percentile</span><span class="v">{{ fmt.rate(data.p95_bps) }}</span><span class="d">what burstable plans bill</span></div>
    </div>
    <section class="panel">
      <header>
        <h2>Data per {{ data.step }}</h2>
        <div class="legend"><span><i style="background: var(--info)"></i>In</span><span><i style="background: var(--accent)"></i>Out</span><span><i style="background: var(--warn)"></i>Relayed</span></div>
      </header>
      <StackedBars :buckets="data.buckets" :step="data.step" />
    </section>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>This month's allowance</h2><span class="small muted">{{ monthLabel }} · alerts at 80%</span></header>
        <div class="stack">
          <div v-for="a in data.allowances" :key="a.server" class="stack" style="gap: 6px">
            <div class="row" style="justify-content: space-between">
              <b>{{ names.get(a.server) || a.server }}</b>
              <span class="num small">{{ fmt.size(a.used) }}{{ a.allowance ? ` of ${fmt.size(a.allowance)}` : '' }}</span>
            </div>
            <Bar v-if="a.allowance" label="" :frac="a.used / a.allowance" :text="fmt.pct(a.used / a.allowance * 100)" />
            <div class="row small muted" style="justify-content: space-between">
              <span>On course for {{ fmt.size(a.projected) }} this month</span>
              <form class="row" style="gap: 6px" @submit.prevent="saveAllowance(a.server)">
                <label class="small" :for="`tb-${a.server}`">Allowance (TB)</label>
                <input :id="`tb-${a.server}`" v-model="allowanceInput[a.server]" inputmode="decimal" placeholder="none" style="width: 80px; padding: 5px 8px">
                <button class="small" type="submit">Save</button>
              </form>
            </div>
          </div>
        </div>
      </section>
      <section class="panel">
        <header><h2>By server</h2></header>
        <div class="stack">
          <div v-for="s in data.by_server" :key="s.server" class="stack" style="gap: 6px">
            <div class="row" style="justify-content: space-between"><b>{{ names.get(s.server) || s.server }}</b><span class="num small">{{ fmt.size(s.rx + s.tx) }}</span></div>
            <div class="split"><div class="in" :style="{ width: `${share(s.rx)}%` }"></div><div class="out" :style="{ width: `${share(s.tx)}%` }"></div></div>
            <span class="small muted">In {{ fmt.size(s.rx) }} · out {{ fmt.size(s.tx) }} · relayed {{ fmt.size(s.relayed) }}</span>
          </div>
          <div v-if="!data.by_server.length" class="empty">No traffic yet.</div>
        </div>
      </section>
    </div>
    <section class="panel">
      <header><h2>{{ data.rows_are === 'months' ? 'Month by month' : 'Day by day' }}</h2><span class="small muted">UTC</span></header>
      <div v-if="data.rows.length" class="table-wrap">
        <table>
          <thead><tr><th>{{ data.rows_are === 'months' ? 'Month' : 'Day' }}</th><th class="r">In</th><th class="r">Out</th><th class="r">Relayed</th><th class="r">Peak</th><th class="r">95th percentile</th><th class="r">Peak players</th></tr></thead>
          <tbody>
            <tr v-for="r in data.rows" :key="r.t">
              <td>{{ rowLabel(r.t) }}</td><td class="r">{{ fmt.size(r.rx) }}</td><td class="r">{{ fmt.size(r.tx) }}</td>
              <td class="r" style="color: var(--warn)">{{ fmt.size(r.relayed) }}</td><td class="r">{{ fmt.rate(r.peak_bps) }}</td>
              <td class="r">{{ fmt.rate(r.p95_bps) }}</td><td class="r">{{ fmt.n(r.peak_players) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No traffic in this period yet.</div>
    </section>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted, reactive, ref, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import StackedBars from '../components/StackedBars.vue';
import Bar from '../components/Bar.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const REPORT_RANGES = [[86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '12 months']];
const range = ref(2592000);
const server = ref('');
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
const { data, error, reload } = useLoad(() => api('GET', `/bandwidth?range=${range.value}${server.value ? `&server=${encodeURIComponent(server.value)}` : ''}`), () => [range.value, server.value]);
const total = computed(() => (data.value ? data.value.totals.rx + data.value.totals.tx : 0));
const rangeLabel = computed(() => ({ 86400: 'Last 24 hours', 604800: 'Last 7 days', 2592000: 'Last 30 days', 31536000: 'Last 12 months' })[range.value]);
const monthLabel = computed(() => data.value && new Date(data.value.month.start * 1000).toLocaleDateString(undefined, { month: 'long', year: 'numeric', timeZone: 'UTC' }));
const share = v => {
  const max = Math.max(1, ...(data.value?.by_server || []).map(s => s.rx + s.tx));
  return (v / max * 100).toFixed(1);
};
const rowLabel = t => new Date(t * 1000).toLocaleDateString(undefined, data.value.rows_are === 'months' ? { month: 'long', year: 'numeric', timeZone: 'UTC' } : { weekday: 'short', day: 'numeric', month: 'short', timeZone: 'UTC' });

const allowanceInput = reactive({});
watch(data, d => {
  for (const a of d?.allowances || []) if (!(a.server in allowanceInput)) allowanceInput[a.server] = a.allowance ? String(a.allowance / 1e12) : '';
});
async function saveAllowance(id) {
  const tb = Number(String(allowanceInput[id] || '0').replace(',', '.'));
  if (!Number.isFinite(tb) || tb < 0) { toast('An allowance is a number of TB (empty for none).', true); return; }
  try {
    await api('PUT', '/allowances', { server: id, tb });
    toast(tb ? `Allowance set: ${tb} TB a month.` : 'Allowance removed.');
    reload();
  } catch (e) { toast(e.message, true); }
}

function exportCsv() {
  const rows = [['period_start_utc', 'in_bytes', 'out_bytes', 'relayed_bytes', 'peak_bits_per_s', 'p95_bits_per_s', 'peak_players']];
  for (const r of [...data.value.rows].reverse()) {
    rows.push([new Date(r.t * 1000).toISOString(), Math.round(r.rx), Math.round(r.tx), Math.round(r.relayed), Math.round((r.peak_bps || 0) * 8), Math.round((r.p95_bps || 0) * 8), r.peak_players || 0]);
  }
  const blob = new Blob([rows.map(r => r.join(',')).join('\n') + '\n'], { type: 'text/csv' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = `bandwidth-${server.value || 'all'}-${range.value / 86400}d.csv`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}
</script>

<style scoped>
.kpis-6 { grid-template-columns: repeat(6, minmax(0, 1fr)); }
@media (max-width: 1100px) { .kpis-6 { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@media (max-width: 820px) { .kpis-6 { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
.split { display: flex; height: 8px; border-radius: 4px; overflow: hidden; background: var(--panel-2); }
.split .in { background: var(--info); }
.split .out { background: var(--accent); }
</style>
