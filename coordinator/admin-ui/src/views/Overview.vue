<template>
  <template v-if="o">
    <PageTop title="Overview" sub="Who's playing, on which servers, and what needs you.">
      <template #stats>
        <Stat :value="fmt.n(players)" label="Players online" :sub="`${fmt.n(inMatch)} in a match`"><Sparkline :values="pulse.series.map(p => p.players)" /></Stat>
        <Stat :value="pulse.now.servers ? fmt.bits(pulse.now.bps) : '–'" label="Bandwidth now" :sub="pulse.now.servers ? `${fmt.bits(pulse.now.relayed)} through the relay` : 'waiting for the servers\' pulse'"><Sparkline :values="pulse.series.map(p => p.bps)" color="var(--info)" /></Stat>
        <Stat :value="fmt.n(Math.max(o.peak_24h || 0, players))" label="Peak, 24 h" sub="most online at once" />
        <Stat :value="fmt.n(sessions)" label="Sessions" :sub="pulse.now.servers ? `${fmt.n(pulse.now.matches)} matches running` : 'lobbies and matches'" />
        <Stat :value="fmt.n(online.length)" :unit="`/ ${o.servers.length}`" label="Servers up" :sub="`Coordinator ${o.coordinator.version} · ${delisted ? `${delisted} delisted` : 'all listed'}`" />
        <RouterLink class="tile-link" to="/inbox"><Stat :value="fmt.n(waiting)" label="Needs you" :tone="waiting ? 'warn' : ''" sub="Open the Inbox →" /></RouterLink>
      </template>
      <span class="pill" :class="health.tone"><i class="dot" :class="health.tone"></i> {{ health.text }}</span>
      <RouterLink class="btn" to="/sessions">Sessions</RouterLink>
    </PageTop>
    <section v-if="(o.alerts || []).length" class="panel">
      <header><h2>Alerts</h2><RouterLink class="small" to="/alerts">Alerts and their settings →</RouterLink></header>
      <ul class="alerts">
        <li v-for="a in o.alerts" :key="a.id" :class="a.level">
          <span class="pill" :class="a.level">{{ a.level === 'bad' ? 'Down' : 'Warning' }}</span>
          <span class="grow">{{ alertText(a.detail) }}</span>
          <span class="small muted nowrap">{{ alertWhere(a, serverNames) ? alertWhere(a, serverNames) + ' · ' : '' }}since {{ fmt.ago(a.started_at) }}</span>
        </li>
      </ul>
    </section>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Players online, 24 hours</h2><div class="legend"><span v-for="s in playerLines" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart v-if="day" :series="playerLines" :from="day.from" :to="day.to" />
        <div v-else class="empty">Loading…</div>
      </section>
      <section class="panel">
        <header><h2>Where players are now</h2><RouterLink class="small" to="/players?tab=report">Players and map →</RouterLink></header>
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
  <template v-else>
    <PageTop title="Overview" sub="Who's playing, on which servers, and what needs you." />
    <div v-if="error" class="panel"><p class="err">{{ error }}</p></div>
    <div v-else class="empty">Loading…</div>
  </template>
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
import Stat from '../components/Stat.vue';
import { alertText, alertWhere, fmt, liveTotals, serverName, seriesFor, totalSeries } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const error = ref('');
onMounted(() => ensureOverview().catch(e => { error.value = e.message; }));
const o = computed(() => live.overview);
const online = computed(() => o.value.servers.filter(sv => sv.online));
// The servers' pulses (every 10 s) when they send them; else their last minute's report.
const pulse = computed(() => liveTotals(live.pulses));
const players = computed(() => (pulse.value.now.servers ? pulse.value.now.players : online.value.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0)));
const inMatch = computed(() => (pulse.value.now.servers ? pulse.value.now.in_match : online.value.reduce((n, sv) => n + (sv.metrics?.players?.in_match || 0), 0)));
const serverNames = computed(() => new Map((o.value?.servers || []).map(sv => [sv.id, serverName(sv)])));
const serverLabel = id => serverName(o.value?.servers.find(sv => sv.id === id) || { id });
const sessions = computed(() => online.value.reduce((n, sv) => n + (sv.metrics?.activity || []).reduce((m, a) => m + (a.sessions || 0), 0), 0));
// What's waiting on an admin (the side menu's Inbox count), and the network's state in a word.
const waiting = computed(() => (o.value.support_open || 0) + (o.value.open_reports || 0) + (o.value.chat_reports || 0) + (o.value.alerts || []).length);
const health = computed(() => {
  const alerts = o.value.alerts || [];
  const down = o.value.servers.length - online.value.length;
  if (alerts.some(a => a.level === 'bad') || down) return { tone: 'bad', text: down ? `${down} server${down === 1 ? '' : 's'} down` : 'Something is down' };
  if (alerts.length) return { tone: 'warn', text: `${alerts.length} warning${alerts.length === 1 ? '' : 's'}` };
  return { tone: 'ok', text: 'All normal' };
});
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
.tile-link { display: contents; color: inherit; text-decoration: none; }
.alerts { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.alerts li { display: flex; gap: 12px; align-items: center; padding: 10px 12px; border-radius: 8px; background: var(--panel-2); }
.alerts li.bad { background: var(--bad-soft); }
.alerts li.warn { background: var(--warn-soft); }
.grow { flex: 1; min-width: 0; }
.nowrap { white-space: nowrap; }
</style>
