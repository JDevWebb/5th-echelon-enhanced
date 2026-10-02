<template>
  <PageTop title="Players" :sub="sub">
    <RangePicker v-model="range" :options="REPORT_RANGES" />
  </PageTop>
  <template v-if="r">
    <div class="kpis kpis-6">
      <div class="kpi"><span class="label">Players</span><span class="v">{{ fmt.n(t.players) }}</span><span class="d">played at least once</span></div>
      <div class="kpi"><span class="label">Daily average</span><span class="v">{{ fmt.n(t.daily_average, 1) }}</span><span class="d">players a day</span></div>
      <div class="kpi"><span class="label">New players</span><span class="v">{{ fmt.n(t.new) }}</span><span class="d">first seen in this period</span></div>
      <div class="kpi"><span class="label">Came back</span><span class="v">{{ t.earlier ? fmt.pct(t.returning / t.earlier * 100) : '–' }}</span><span class="d">{{ t.earlier ? `${fmt.n(t.returning)} of the ${fmt.n(t.earlier)} before` : 'nothing to compare yet' }}</span></div>
      <div class="kpi"><span class="label">Time played</span><span class="v">{{ fmt.n(t.minutes_per_player_day) }}<small>min</small></span><span class="d">per player, per day played</span></div>
      <div class="kpi"><span class="label">Peak online</span><span class="v">{{ fmt.n(t.peak) }}</span><span class="d">most at once</span></div>
    </div>
    <p class="small muted">Players are counted per server, from ids each server makes unreadable with its own key: the coordinator never learns who they are, and someone playing on two servers counts twice.{{ r.tracking_since ? ` Counted since ${new Date(r.tracking_since * 1000).toLocaleDateString(undefined, { dateStyle: 'medium', timeZone: 'UTC' })}.` : '' }}</p>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Players per day</h2><div class="legend"><span v-for="s in daySeries" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart :series="daySeries" :from="r.from" :to="r.to" />
      </section>
      <section class="panel">
        <header><h2>Sign-ins per day</h2><div class="legend"><span v-for="s in signinSeries" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart :series="signinSeries" :from="r.from" :to="r.to" />
      </section>
    </div>
    <section class="panel">
      <header><h2>When people play</h2><span class="small muted">Average players online, in your time zone</span></header>
      <Heatmap v-if="r.hours.length" :hours="r.hours" />
      <div v-else class="empty">Not enough history yet.</div>
    </section>
    <section class="panel">
      <header><h2>Pings by city</h2><span class="small muted">As players' launchers measured them · median</span></header>
      <div v-if="r.cities.length" class="table-wrap">
        <table>
          <thead><tr><th>City</th><th class="r">Reports</th><th v-for="sv in servers" :key="sv.id" class="r">{{ serverName(sv) }}</th></tr></thead>
          <tbody>
            <tr v-for="c in r.cities" :key="c.country + c.city">
              <td>{{ c.country ? flag(c.country) + ' ' : '' }}{{ c.city || c.country || 'Unknown' }}</td>
              <td class="r">{{ fmt.n(c.reports) }}</td>
              <td v-for="sv in servers" :key="sv.id" class="r" :style="{ color: pingColor(c.median[sv.id]) }">{{ c.median[sv.id] == null ? '–' : fmt.ms(c.median[sv.id]) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No launcher has reported pings in this period.</div>
    </section>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>

  <PageTop title="Map" :sub="mapRange === 0 ? 'Players online now, by city.' : 'Time played, by city (player-minutes).'">
    <RangePicker v-model="mapRange" :options="PLACE_RANGES" />
  </PageTop>
  <template v-if="places">
    <section class="panel">
      <WorldMap :places="places.places" :servers="servers" :unit="places.unit" />
      <div class="row" style="justify-content: space-between; margin-top: 8px">
        <span class="legend"><span><i style="background: var(--accent)"></i>{{ places.unit === 'players' ? 'players' : 'player-minutes' }}</span><span><i style="background: var(--warn)"></i>servers</span></span>
        <span class="attr">{{ places.attribution }}</span>
      </div>
    </section>
    <section class="panel">
      <header><h2>By city</h2><span class="muted small">{{ fmt.n(placeTotal) }} {{ places.unit }} · {{ places.places.length }} places</span></header>
      <div v-if="places.places.length" class="table-wrap">
        <table>
          <thead><tr><th>City</th><th>Region</th><th>Country</th><th class="r">{{ places.unit === 'players' ? 'Players' : 'Minutes' }}</th><th class="r">Share</th><th>Servers</th></tr></thead>
          <tbody>
            <tr v-for="(p, i) in places.places" :key="i">
              <td>{{ p.city || (p.country ? '–' : 'Unknown or private network') }}</td>
              <td class="muted">{{ p.region || '–' }}</td>
              <td>{{ p.country ? `${flag(p.country)} ${p.country_name || p.country}` : '–' }}</td>
              <td class="r">{{ fmt.n(p.amount) }}</td>
              <td class="r">{{ fmt.pct(placeTotal ? p.amount / placeTotal * 100 : 0) }}</td>
              <td class="muted small">{{ Object.entries(p.servers || {}).sort((a, b) => b[1] - a[1]).map(([sid, n]) => `${names.get(sid) || sid} ${fmt.n(n)}`).join(' · ') }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">{{ mapRange === 0 ? 'Nobody is playing right now.' : 'No players in this period.' }}</div>
    </section>
  </template>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import LineChart from '../components/LineChart.vue';
import Heatmap from '../components/Heatmap.vue';
import WorldMap from '../components/WorldMap.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { flag, fmt, PALETTE, PLACE_RANGES, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const REPORT_RANGES = [[86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '12 months']];
const range = ref(2592000);
const mapRange = ref(0);
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
const { data: r, error } = useLoad(() => api('GET', `/players-report?range=${range.value}`), () => range.value);
const { data: places } = useLoad(() => api('GET', `/places?range=${mapRange.value}`), () => mapRange.value);
const t = computed(() => r.value.totals);
const sub = computed(() => `${({ 86400: 'Last 24 hours', 604800: 'Last 7 days', 2592000: 'Last 30 days', 31536000: 'Last 12 months' })[range.value]} · who plays, when, and from where`);
const daySeries = computed(() => [
  { name: 'Players', color: PALETTE[0], points: r.value.days.map(d => ({ t: d.t, v: d.players })) },
  { name: 'New', color: PALETTE[1], points: r.value.days.map(d => ({ t: d.t, v: d.new })) },
]);
const signinSeries = computed(() => [
  { name: 'Signed in', color: PALETTE[0], points: r.value.signins.map(d => ({ t: d.t, v: d.ok })) },
  { name: 'Refused', color: PALETTE[4], points: r.value.signins.map(d => ({ t: d.t, v: d.failed })) },
  { name: 'New accounts', color: PALETTE[1], points: r.value.signins.map(d => ({ t: d.t, v: d.new })) },
]);
const placeTotal = computed(() => (places.value?.places || []).reduce((n, p) => n + p.amount, 0));
const pingColor = ms => (ms == null ? 'var(--faint)' : ms < 90 ? 'var(--ok)' : ms < 180 ? 'var(--text)' : 'var(--warn)');
</script>

<style scoped>
.kpis-6 { grid-template-columns: repeat(6, minmax(0, 1fr)); }
@media (max-width: 1100px) { .kpis-6 { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@media (max-width: 820px) { .kpis-6 { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
