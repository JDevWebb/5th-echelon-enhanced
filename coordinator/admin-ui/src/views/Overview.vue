<template>
  <template v-if="o">
    <PageTop title="Overview" :sub="`Coordinator ${o.coordinator.version} · ${online.length} of ${o.servers.length} servers online`" />
    <RouterLink v-if="(o.alerts || []).length" to="/alerts" class="callout" :class="o.alerts.some(a => a.level === 'bad') ? 'bad' : 'warn'" style="text-decoration: none; color: inherit">
      <b>{{ o.alerts.length === 1 ? '1 alert' : `${o.alerts.length} alerts` }}:</b> {{ o.alerts[0].detail }}{{ o.alerts.length > 1 ? ' …' : '' }}
    </RouterLink>
    <div class="kpis">
      <div class="kpi"><span class="label">Players online</span><span class="v" :key="'p' + players" :class="{ fresh: true }">{{ fmt.n(players) }}</span><span class="d">{{ fmt.n(inMatch) }} in a match</span><Sparkline :values="pulse.series.map(p => p.players)" /></div>
      <div class="kpi"><span class="label">Bandwidth now</span><span class="v">{{ pulse.now.servers ? fmt.bits(pulse.now.bps) : '–' }}</span><span class="d">{{ pulse.now.servers ? `${fmt.bits(pulse.now.relayed)} through the relay` : 'waiting for the servers\' pulse' }}</span><Sparkline :values="pulse.series.map(p => p.bps)" color="var(--info)" /></div>
      <div class="kpi"><span class="label">Peak, 24 h</span><span class="v">{{ fmt.n(Math.max(o.peak_24h || 0, players)) }}</span><span class="d">most online at once</span></div>
      <div class="kpi"><span class="label">Sessions</span><span class="v">{{ fmt.n(sessions) }}</span><span class="d">{{ pulse.now.servers ? `${fmt.n(pulse.now.matches)} matches running` : 'lobbies and matches' }}</span></div>
      <div class="kpi"><span class="label">Servers</span><span class="v">{{ fmt.n(online.length) }}<small>/ {{ o.servers.length }}</small></span><span class="d">{{ delisted ? `${delisted} delisted` : 'all listed' }}</span></div>
    </div>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Players online, 24 hours</h2><div class="legend"><span v-for="s in playerLines" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart v-if="day" :series="playerLines" :from="day.from" :to="day.to" />
        <div v-else class="empty">Loading…</div>
      </section>
      <section class="panel">
        <header><h2>Where players are now</h2><RouterLink class="small" to="/players?tab=report">Players &amp; map →</RouterLink></header>
        <WorldMap :places="now?.places || []" :servers="o.servers" unit="players" live />
        <p class="attr">{{ now?.attribution }}</p>
      </section>
    </div>
    <section class="panel">
      <header><h2>Servers</h2><span class="small muted">Release {{ o.rollout.target || '–' }} · {{ stage }}</span><RouterLink class="small" to="/servers">Details →</RouterLink></header>
      <ServerTable :overview="o" />
    </section>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Live events</h2><span class="small muted">From the servers, every 10 s · counts only</span></header>
        <ul v-if="live.feed.length" class="feed">
          <li v-for="e in live.feed.slice(0, 8)" :key="`${e.t}-${e.server}-${e.kind}-${e.text}`" :class="{ fresh: e.fresh }">
            <span class="row" style="gap: 8px"><span class="dot" :class="e.level === 'warn' ? 'warn' : e.level === 'muted' ? '' : 'ok'"></span><b>{{ e.text }}</b></span>
            <span class="small muted">{{ serverLabel(e.server) }} · {{ fmt.ago(e.t) }}</span>
          </li>
        </ul>
        <div v-else class="empty">Nothing yet. Sign-ins, new accounts and matches show here as they happen.</div>
      </section>
      <section class="panel">
        <header><h2>Admin activity</h2><RouterLink class="small" to="/audit">Audit log →</RouterLink></header>
        <ul v-if="recent.length" class="feed">
          <li v-for="e in recent" :key="e.id" :class="{ fresh: e.fresh }">
            <span><b>{{ e.event }}</b> <span class="muted">{{ e.admin || 'console' }}</span></span>
            <span class="small muted">{{ e.detail }}{{ e.detail ? ' · ' : '' }}{{ fmt.ago(e.at) }}</span>
          </li>
        </ul>
        <div v-else class="empty">Nothing yet.</div>
      </section>
    </div>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import LineChart from '../components/LineChart.vue';
import WorldMap from '../components/WorldMap.vue';
import ServerTable from '../components/ServerTable.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import Sparkline from '../components/Sparkline.vue';
import { fmt, liveTotals, serverName, seriesFor, totalSeries } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const error = ref('');
onMounted(() => ensureOverview().catch(e => { error.value = e.message; }));
const o = computed(() => live.overview);
const online = computed(() => o.value.servers.filter(sv => sv.online));
// The servers' pulses (every 10 s) when they send them; else their last minute's report.
const pulse = computed(() => liveTotals(live.pulses));
const players = computed(() => (pulse.value.now.servers ? pulse.value.now.players : online.value.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0)));
const inMatch = computed(() => (pulse.value.now.servers ? pulse.value.now.in_match : online.value.reduce((n, sv) => n + (sv.metrics?.players?.in_match || 0), 0)));
const serverLabel = id => serverName(o.value?.servers.find(sv => sv.id === id) || { id });
const sessions = computed(() => online.value.reduce((n, sv) => n + (sv.metrics?.activity || []).reduce((m, a) => m + (a.sessions || 0), 0), 0));
const delisted = computed(() => o.value.servers.filter(sv => sv.delisted).length);
const stage = computed(() => {
  const r = o.value.rollout;
  return r.target ? `${r.target} · ${r.paused ? 'paused' : r.stage}` : 'no release yet';
});

const { data: day } = useLoad(() => api('GET', '/series?range=86400'));
const { data: now } = useLoad(() => api('GET', '/places?range=0'));
const playerLines = computed(() => {
  if (!day.value) return [];
  const lines = seriesFor(day.value, 'players', o.value?.servers || []);
  if (lines.length > 1) lines.unshift(totalSeries(day.value, 'players'));
  return lines;
});

// The audit log's newest entries: what came in live, then the stored ones.
const { data: stored } = useLoad(() => api('GET', '/audit'), () => null, { refresh: false });
const recent = computed(() => {
  const seen = new Set(live.audit.map(e => e.id));
  const fresh = live.audit.map(e => ({ ...e, fresh: true }));
  return [...fresh, ...(stored.value?.events || []).filter(e => !seen.has(e.id))].slice(0, 8);
});
</script>

<style scoped>
.feed { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; }
.feed li { display: flex; flex-direction: column; gap: 2px; padding: 8px 6px; border-bottom: 1px solid var(--line); border-radius: 6px; }
.feed li:last-child { border-bottom: none; }
</style>
