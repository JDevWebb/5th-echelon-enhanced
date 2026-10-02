<template>
  <template v-if="o">
    <PageTop title="Overview" :sub="`Coordinator ${o.coordinator.version} · ${online.length} of ${o.servers.length} servers online`" />
    <div class="kpis">
      <div class="kpi"><span class="label">Players online</span><span class="v" :key="'p' + players" :class="{ fresh: true }">{{ fmt.n(players) }}</span><span class="d">{{ fmt.n(inMatch) }} in a match</span></div>
      <div class="kpi"><span class="label">Peak, 24 h</span><span class="v">{{ fmt.n(Math.max(o.peak_24h || 0, players)) }}</span><span class="d">most online at once</span></div>
      <div class="kpi"><span class="label">Sessions</span><span class="v">{{ fmt.n(sessions) }}</span><span class="d">lobbies and matches</span></div>
      <div class="kpi"><span class="label">Servers</span><span class="v">{{ fmt.n(online.length) }}<small>/ {{ o.servers.length }}</small></span><span class="d">{{ delisted ? `${delisted} delisted` : 'all listed' }}</span></div>
      <div class="kpi"><span class="label">Release</span><span class="v" style="font-size: 18px">{{ o.rollout.target || '–' }}</span><span class="d">{{ stage }}</span></div>
    </div>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Players online, 24 hours</h2><div class="legend"><span v-for="s in playerLines" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart v-if="day" :series="playerLines" :from="day.from" :to="day.to" />
        <div v-else class="empty">Loading…</div>
      </section>
      <section class="panel">
        <header><h2>Where players are now</h2><RouterLink class="small" to="/players">Players &amp; map →</RouterLink></header>
        <WorldMap :places="now?.places || []" :servers="o.servers" unit="players" />
        <p class="attr">{{ now?.attribution }}</p>
      </section>
    </div>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Servers</h2><RouterLink class="small" to="/servers">Details →</RouterLink></header>
        <ServerTable :overview="o" />
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
import { fmt, seriesFor, totalSeries } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const error = ref('');
onMounted(() => ensureOverview().catch(e => { error.value = e.message; }));
const o = computed(() => live.overview);
const online = computed(() => o.value.servers.filter(sv => sv.online));
const players = computed(() => online.value.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0));
const inMatch = computed(() => online.value.reduce((n, sv) => n + (sv.metrics?.players?.in_match || 0), 0));
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
