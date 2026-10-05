<template>
  <PageTop title="Players" :sub="sub">
    <template #stats>
      <template v-if="tab === 'report' && r">
        <Stat :value="fmt.n(t.players)" label="Players" sub="played at least once" />
        <Stat :value="fmt.n(t.daily_average, 1)" label="Daily average" sub="players a day" />
        <Stat :value="fmt.n(t.new)" label="New players" sub="first seen in this period" />
        <Stat :value="t.earlier ? fmt.pct(t.returning / t.earlier * 100) : '–'" label="Came back" :sub="t.earlier ? `${fmt.n(t.returning)} of the ${fmt.n(t.earlier)} before` : 'nothing to compare yet'" />
        <Stat :value="fmt.n(t.minutes_per_player_day)" unit="min" label="Time played" sub="per player, per day played" />
        <Stat :value="fmt.n(t.peak)" label="Peak online" sub="most at once" />
      </template>
      <template v-else>
        <Stat :value="fmt.n(onlineNow.players)" label="Online now" :sub="`${fmt.n(onlineNow.inMatch)} in a match`" />
        <Stat :value="fmt.n(servers.length)" label="Servers" sub="each with its own accounts" />
      </template>
    </template>
    <div class="seg" role="group" aria-label="View">
      <button v-for="[id, label] in TABS" :key="id" type="button" :aria-pressed="String(tab === id)" @click="setTab(id)">{{ label }}</button>
    </div>
    <RangePicker v-if="tab === 'report'" v-model="range" :options="REPORT_RANGES" />
    <RangePicker v-if="tab === 'matches'" v-model="matchDays" :options="MATCH_RANGES" />
  </PageTop>
  <PlayerList v-if="tab === 'accounts'" />
  <MatchesReport v-else-if="tab === 'matches'" :days="matchDays" />
  <Leaderboards v-else-if="tab === 'leaderboards'" />
  <template v-else-if="r">
    <p class="small muted">Players are counted per server: someone playing on two servers counts twice. Time played comes from play sessions for servers that send them ({{ sessionServers }}), else from a sample each minute.{{ r.tracking_since ? ` Counted since ${new Date(r.tracking_since * 1000).toLocaleDateString(undefined, { dateStyle: 'medium', timeZone: 'UTC' })}.` : '' }}</p>
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

  <template v-if="tab === 'report'">
    <section class="panel">
      <header>
        <div>
          <h2 class="big-title">Map</h2>
          <p class="small muted">{{ mapRange === 0 ? 'Players online now, by city.' : 'Time played, by city (player-minutes).' }}</p>
        </div>
        <RangePicker v-model="mapRange" :options="PLACE_RANGES" />
      </header>
      <template v-if="places">
        <WorldMap :places="places.places" :servers="servers" :unit="places.unit" :live="mapRange === 0" />
        <div class="row" style="justify-content: space-between; margin-top: 8px">
          <span v-if="mapRange === 0" class="legend"><span><i style="background: var(--accent)"></i>in a match</span><span><i style="background: var(--info)"></i>in a lobby</span><span><i style="background: var(--muted)"></i>in the menus</span><span><i style="background: var(--warn)"></i>servers</span><span><i style="background: #02070a; opacity: 0.5"></i>night</span></span>
          <span v-else class="legend"><span><i style="background: var(--accent)"></i>{{ places.unit === 'players' ? 'players' : 'player-minutes' }}</span><span><i style="background: var(--warn)"></i>servers</span><span><i style="background: #02070a; opacity: 0.5"></i>night now</span></span>
          <span class="attr">{{ places.attribution }}</span>
        </div>
      </template>
      <div v-else class="empty">Loading…</div>
    </section>
  <template v-if="places">
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
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import PlayerList from '../components/PlayerList.vue';
import MatchesReport from '../components/MatchesReport.vue';
import Leaderboards from '../components/Leaderboards.vue';
import RangePicker from '../components/RangePicker.vue';
import Stat from '../components/Stat.vue';
import LineChart from '../components/LineChart.vue';
import Heatmap from '../components/Heatmap.vue';
import WorldMap from '../components/WorldMap.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { flag, fmt, PALETTE, PLACE_RANGES, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const REPORT_RANGES = [[86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '12 months']];
const MATCH_RANGES = [[1, 'Today'], [7, '7 d'], [30, '30 d'], [365, '12 months']];
const TABS = [['accounts', 'Accounts'], ['report', 'Report and map'], ['matches', 'Matches'], ['leaderboards', 'Leaderboards']];
const route = useRoute();
const router = useRouter();
const tab = computed(() => (TABS.some(([id]) => id === route.query.tab) ? route.query.tab : 'accounts'));
const setTab = id => router.replace({ query: { ...route.query, tab: id } });
const range = ref(2592000);
const matchDays = ref(30);
const mapRange = ref(0);
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
// The report and map load while their tab is open.
const onReport = computed(() => tab.value === 'report');
const { data: r, error } = useLoad(() => (onReport.value ? api('GET', `/players-report?range=${range.value}`) : null), () => [range.value, onReport.value]);
const { data: places } = useLoad(() => (onReport.value ? api('GET', `/places?range=${mapRange.value}`) : null), () => [mapRange.value, onReport.value]);
const sessionServers = computed(() => {
  const ids = Object.keys(r.value?.sessions_from || {});
  return ids.length ? ids.map(id => names.value.get(id) || id).join(', ') : 'none yet';
});
const t = computed(() => r.value.totals);
const onlineNow = computed(() => {
  const up = servers.value.filter(sv => sv.online);
  return { players: up.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0), inMatch: up.reduce((n, sv) => n + (sv.metrics?.players?.in_match || 0), 0) };
});
const sub = computed(() => ({
  accounts: 'Every server\'s accounts: find a player, see what they played, and act on their account.',
  matches: 'Finished matches: by mode, map and game mode, how long, how many play.',
  leaderboards: 'The global leaderboards: everyone across the network, one entry per person.',
}[tab.value] || `${({ 86400: 'Last 24 hours', 604800: 'Last 7 days', 2592000: 'Last 30 days', 31536000: 'Last 12 months' })[range.value]} · who plays, when, and from where`));
const daySeries = computed(() => [
  { name: 'Players', color: PALETTE[0], points: r.value.days.map(d => ({ t: d.t, v: d.players })) },
  { name: 'New', color: PALETTE[1], points: r.value.days.map(d => ({ t: d.t, v: d.new })) },
]);
const signinSeries = computed(() => [
  { name: 'Signed in', color: PALETTE[0], points: r.value.signins.map(d => ({ t: d.t, v: d.ok })) },
  { name: 'Refused', color: PALETTE[4], points: r.value.signins.map(d => ({ t: d.t, v: d.failed })) },
  { name: 'New accounts', color: PALETTE[1], points: r.value.signins.map(d => ({ t: d.t, v: d.new })) },
  { name: 'Failed joins', color: PALETTE[2], points: r.value.signins.map(d => ({ t: d.t, v: d.failed_joins || 0 })) },
]);
const placeTotal = computed(() => (places.value?.places || []).reduce((n, p) => n + p.amount, 0));
const pingColor = ms => (ms == null ? 'var(--faint)' : ms < 90 ? 'var(--ok)' : ms < 180 ? 'var(--text)' : 'var(--warn)');
</script>

