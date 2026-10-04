<template>
  <PageTop title="Sessions" sub="Each player's time on the servers: when they were on, the rooms they were in and with whom, and anything that went wrong.">
    <div class="seg" role="group" aria-label="Server">
      <button type="button" :aria-pressed="String(server === '')" @click="server = ''">All servers</button>
      <button v-for="sv in servers" :key="sv.id" type="button" :aria-pressed="String(server === sv.id)" @click="server = sv.id">{{ sv.name }}</button>
    </div>
    <RangePicker v-model="hours" :options="RANGES" />
  </PageTop>
  <div v-if="error && !data" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else-if="!data" class="empty">Loading…</div>
  <template v-else>
    <div class="kpis">
      <div class="kpi"><span class="label">Players</span><span class="v">{{ fmt.n(data.players.filter(p => p.online.length).length) }}</span><span class="d">on in this period</span></div>
      <div class="kpi"><span class="label">In a match</span><span class="v">{{ fmt.n(matchPlayers) }}</span><span class="d">played with someone</span></div>
      <div class="kpi"><span class="label">Problems</span><span class="v" :class="{ bad: counts.bad }">{{ fmt.n(counts.bad) }}</span><span class="d">{{ fmt.n(counts.warn) }} warnings · {{ fmt.n(counts.info) }} notes</span></div>
      <div class="kpi"><span class="label">Server trouble</span><span class="v" :class="{ warn: data.outages.length }">{{ fmt.n(data.outages.length) }}</span><span class="d">alerts and unanswered pings</span></div>
    </div>

    <section class="panel">
      <header>
        <h2>Problems</h2>
        <div class="seg" role="group" aria-label="Show">
          <button v-for="[id, label] in LEVELS" :key="id" type="button" :aria-pressed="String(level === id)" @click="level = id">{{ label }}</button>
        </div>
      </header>
      <div v-if="shownProblems.length" class="stack">
        <button v-for="(p, i) in shownProblems" :key="i" type="button" class="callout problem" :class="p.level" @click="pick(p.server, p.player, p.name)">
          <div class="row" style="justify-content: space-between">
            <b>{{ p.title }}</b>
            <span class="small muted">{{ time(p.at) }} · {{ serverLabel(p.servers ? p.servers.join(' + ') : p.server) }}</span>
          </div>
          <span class="small">{{ p.text }}</span>
        </button>
      </div>
      <div v-else class="empty">{{ level === 'all' ? 'Nothing went wrong in this period.' : 'Nothing at this level.' }}</div>
    </section>

    <section v-if="data.outages.length" class="panel">
      <header><h2>Server trouble</h2><span class="small muted">shaded on the timeline</span></header>
      <div class="table-wrap">
        <table>
          <thead><tr><th>What</th><th>Server</th><th>From</th><th class="r">For</th></tr></thead>
          <tbody>
            <tr v-for="(o, i) in data.outages" :key="i">
              <td><span class="dot" :class="o.level === 'bad' ? 'bad' : 'warn'"></span> {{ o.text }}</td>
              <td class="muted">{{ o.server ? serverLabel(o.server) : 'all' }}</td>
              <td class="small muted">{{ fmt.when(o.from) }}</td>
              <td class="r">{{ o.to ? fmt.dur(o.to - o.from) || '<1m' : 'still' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section class="panel">
      <header>
        <h2>Timeline</h2>
        <div class="legend">
          <span><i class="sw online"></i>Online</span>
          <span><i class="sw party"></i>Party</span>
          <span><i class="sw coop"></i>Co-op match</span>
          <span><i class="sw svm"></i>Spies vs Mercs</span>
          <span><i class="sw alone"></i>Match, alone</span>
          <span><i class="tick bad"></i>Problem</span>
          <span><i class="tick info"></i>Search</span>
          <span><i class="tick ok"></i>Stats</span>
        </div>
      </header>
      <div v-if="groups.length" class="tl-wrap">
        <div class="tl">
          <div class="tl-axis">
            <span class="tl-name"></span>
            <div class="tl-track">
              <span v-for="t in ticks" :key="t" class="tl-tick" :style="{ left: x(t) + '%' }">{{ tickLabel(t) }}</span>
            </div>
          </div>
          <template v-for="g in groups" :key="g.server">
            <div class="tl-server">{{ serverLabel(g.server) }}</div>
            <div v-for="p in g.players" :key="p.key" class="tl-row" :class="{ picked: picked === p.key }" @click="picked = picked === p.key ? '' : p.key">
              <span class="tl-name" :title="p.name">{{ p.name }}</span>
              <div class="tl-track">
                <span v-for="(o, i) in outagesOf(g.server)" :key="'o' + i" class="tl-outage" :style="span(o.from, o.to)" :title="o.text"></span>
                <span v-for="(o, i) in p.online" :key="'n' + i" class="tl-online" :style="span(o[0], o[1])" :title="`Online ${time(o[0])}–${o[1] ? time(o[1]) : 'now'}`"></span>
                <span v-for="(r, i) in p.rooms" :key="'r' + i" class="tl-room" :class="roomClass(r)" :style="span(r.from, r.to)" :title="roomTitle(r)"></span>
                <span v-for="(m, i) in p.marks" :key="'m' + i" class="tl-mark" :class="markClass(m)" :style="{ left: x(m.at) + '%' }" :title="`${time(m.at)} ${m.text}`"></span>
              </div>
            </div>
          </template>
        </div>
      </div>
      <div v-else class="empty">Nobody played in this period.</div>
    </section>

    <section v-if="pickedPlayer" class="panel">
      <header><h2>{{ pickedPlayer.name }}</h2><span class="small muted">{{ serverLabel(pickedPlayer.server) }}</span></header>
      <div class="table-wrap">
        <table>
          <thead><tr><th>When</th><th>What</th></tr></thead>
          <tbody>
            <tr v-for="(row, i) in pickedRows" :key="i">
              <td class="small muted nowrap">{{ time(row.at) }}</td>
              <td><span class="dot" :class="row.cls"></span> {{ row.text }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </template>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const RANGES = [[6, '6 h'], [24, '24 h'], [72, '3 d'], [168, '7 d']];
const LEVELS = [['bad', 'Problems'], ['warn', 'And warnings'], ['all', 'Everything']];
const MODES = { coop: 'co-op', svm: 'Spies vs Mercs' };
const PROBLEM_KINDS = new Set(['signin_refused', 'join_failed', 'relay_drop', 'request_error']);

const hours = ref(24);
const server = ref('');
const level = ref('warn');
const picked = ref('');
onMounted(() => ensureOverview().catch(() => {}));

const { data, error } = useLoad(() => {
  const to = Math.floor(Date.now() / 1000);
  const q = new URLSearchParams({ from: String(to - hours.value * 3600), to: String(to) });
  if (server.value) q.set('server', server.value);
  return api('GET', `/sessions?${q}`);
}, () => [hours.value, server.value]);

const names = computed(() => {
  const m = new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)]));
  for (const sv of data.value?.servers || []) if (sv.name && !m.has(sv.id)) m.set(sv.id, sv.name);
  return m;
});
const servers = computed(() => (data.value?.servers || []).map(sv => ({ id: sv.id, name: names.value.get(sv.id) || sv.id })));
const serverLabel = id => names.value.get(id) || id;

const counts = computed(() => {
  const c = { bad: 0, warn: 0, info: 0 };
  for (const p of data.value?.problems || []) c[p.level] = (c[p.level] || 0) + 1;
  return c;
});
const shownProblems = computed(() => (data.value?.problems || []).filter(p => level.value === 'all' || p.level === 'bad' || (level.value === 'warn' && p.level === 'warn')));
const matchPlayers = computed(() => (data.value?.players || []).filter(p => p.rooms.some(r => r.kind === 'match' && r.with.length)).length);

const span0 = computed(() => data.value?.from || 0);
const span1 = computed(() => data.value?.to || 1);
const x = t => Math.min(100, Math.max(0, (t - span0.value) / (span1.value - span0.value) * 100));
function span(from, to) {
  const a = x(from), b = x(to ?? span1.value);
  return { left: a + '%', width: Math.max(0.25, b - a) + '%' };
}
const ticks = computed(() => {
  const len = span1.value - span0.value;
  const step = [3600, 3 * 3600, 6 * 3600, 12 * 3600, 86400].find(s => len / s <= 8) || 86400;
  const out = [];
  for (let t = Math.ceil(span0.value / step) * step; t < span1.value; t += step) out.push(t);
  return out;
});
const tickLabel = t => new Date(t * 1000).toLocaleString(undefined, (span1.value - span0.value) > 86400 ? { weekday: 'short', hour: 'numeric' } : { hour: 'numeric', minute: '2-digit' });
const time = t => new Date(t * 1000).toLocaleString(undefined, (span1.value - span0.value) > 86400 ? { weekday: 'short', hour: '2-digit', minute: '2-digit' } : { hour: '2-digit', minute: '2-digit', second: '2-digit' });

const keyOf = p => `${p.server}/${p.player ?? 'name:' + p.name}`;
const groups = computed(() => {
  const by = new Map();
  for (const p of data.value?.players || []) {
    if (!by.has(p.server)) by.set(p.server, []);
    by.get(p.server).push({ ...p, key: keyOf(p) });
  }
  const first = p => Math.min(...p.online.map(o => o[0]), ...p.rooms.map(r => r.from), ...p.marks.map(m => m.at));
  return [...by.entries()]
    .map(([server, players]) => ({ server, players: players.sort((a, b) => first(a) - first(b)) }))
    .sort((a, b) => serverLabel(a.server).localeCompare(serverLabel(b.server)));
});
const outagesOf = sv => (data.value?.outages || []).filter(o => !o.server || o.server === sv);

function roomClass(r) {
  if (r.kind !== 'match') return 'party';
  return [r.mode || 'coop', r.with.length ? '' : 'alone', r.private ? 'private' : ''];
}
function roomTitle(r) {
  const what = r.kind === 'match' ? `${r.private ? 'Private' : 'Public'} ${MODES[r.mode] || ''} match` : 'Party';
  const whose = r.host ? 'hosting' : 'joined';
  const who = r.with.length ? ` with ${r.with.join(', ')}` : ', alone';
  return `${what} (${whose})${who}\n${time(r.from)}–${time(r.to)} · ${fmt.dur(r.to - r.from) || '<1m'}`;
}
function markClass(m) {
  if (PROBLEM_KINDS.has(m.kind)) return m.kind === 'request_error' ? 'warn' : 'bad';
  if (m.kind === 'stats') return 'ok';
  if (m.kind === 'search') return 'info';
  return 'muted';
}

function pick(sv, player, name) {
  picked.value = `${sv}/${player ?? 'name:' + name}`;
}
const pickedPlayer = computed(() => groups.value.flatMap(g => g.players).find(p => p.key === picked.value));
const pickedRows = computed(() => {
  const p = pickedPlayer.value;
  if (!p) return [];
  const rows = [
    ...p.online.flatMap(o => [{ at: o[0], text: 'Came online', cls: 'ok' }, ...(o[1] ? [{ at: o[1], text: 'Went offline', cls: '' }] : [])]),
    ...p.rooms.flatMap(r => [
      { at: r.from, text: `${r.host ? 'Opened' : 'Joined'} ${roomTitle(r).split('\n')[0].replace(/^./, c => c.toLowerCase())}`, cls: r.kind === 'match' ? 'ok' : '' },
      { at: r.to, text: `Left the ${r.kind === 'match' ? 'match' : 'party'} after ${fmt.dur(r.to - r.from) || '<1m'}`, cls: '' },
    ]),
    ...p.marks.map(m => ({ at: m.at, text: m.text, cls: markClass(m) })),
  ];
  return rows.sort((a, b) => a.at - b.at);
});
</script>

<style scoped>
.kpi .v.bad { color: var(--bad); }
.kpi .v.warn { color: var(--warn); }
.problem { display: block; width: 100%; text-align: left; color: var(--text); cursor: pointer; }
.problem { border-left: 3px solid var(--faint); }
.problem.bad { border-left-color: var(--bad); }
.problem.warn { border-left-color: var(--warn); }
.problem.info { background: var(--panel-2); border-left-color: var(--info); }
.problem:hover { border-color: var(--line-2); }
.legend { display: flex; flex-wrap: wrap; gap: 12px; font-size: 12px; color: var(--muted); }
.legend span { display: inline-flex; align-items: center; gap: 6px; }
.sw { display: inline-block; width: 16px; height: 8px; border-radius: 2px; }
.tick { display: inline-block; width: 3px; height: 12px; border-radius: 1px; }
.nowrap { white-space: nowrap; }

.tl-wrap { overflow-x: auto; }
.tl { min-width: 720px; font-size: 12px; }
.tl-axis, .tl-row { display: grid; grid-template-columns: 140px 1fr; align-items: center; }
.tl-axis { height: 22px; color: var(--faint); }
.tl-server { margin: 10px 0 4px; font-weight: 600; color: var(--soft); font-size: 12px; }
.tl-row { height: 26px; border-radius: 6px; cursor: pointer; }
.tl-row:hover, .tl-row.picked { background: var(--panel-2); }
.tl-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding: 0 10px 0 4px; color: var(--soft); }
.tl-track { position: relative; height: 100%; }
.tl-tick { position: absolute; top: 4px; transform: translateX(-50%); white-space: nowrap; }
.tl-outage { position: absolute; top: 0; bottom: 0; background: var(--warn-soft); }
.tl-online { position: absolute; top: 11px; height: 4px; border-radius: 2px; background: var(--line-2); }
.tl-room { position: absolute; top: 6px; height: 14px; border-radius: 3px; }
.tl-mark { position: absolute; top: 3px; width: 3px; height: 20px; margin-left: -1px; border-radius: 1px; }

.sw.online { height: 4px; background: var(--line-2); }
.sw.party, .tl-room.party { background: transparent; border: 1px solid var(--muted); }
.sw.coop, .tl-room.coop { background: var(--info); }
.sw.svm, .tl-room.svm { background: var(--accent); }
.tl-room.alone { opacity: 0.45; }
.sw.alone { background: var(--info); opacity: 0.45; }
.tl-room.private { background-image: repeating-linear-gradient(135deg, transparent 0 4px, rgba(0, 0, 0, 0.18) 4px 7px); }
.tick.bad, .tl-mark.bad { background: var(--bad); }
.tl-mark.warn { background: var(--warn); }
.tick.info, .tl-mark.info { background: var(--info); opacity: 0.8; }
.tick.ok, .tl-mark.ok { background: var(--ok); }
.tl-mark.muted { background: var(--faint); opacity: 0.6; }
.dot.info { background: var(--info); }
</style>
