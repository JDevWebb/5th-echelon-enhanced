<template>
  <PageTop title="Live" sub="Every game open now, on every server: what it is, who's in it and how they're connected. Refreshes every 10 seconds.">
    <template #stats>
      <Stat :value="fmt.n(matches.length)" label="Matches" :sub="`${fmt.n(parties.length)} parties`" />
      <Stat :value="fmt.n(inMatches)" label="In a match" :sub="`of ${fmt.n(players.length)} online`" />
      <Stat :value="fmt.n(relayed)" label="Relayed" sub="players through a server's relay" />
    </template>
  </PageTop>
  <div v-if="error && !data" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else-if="!data" class="empty">Loading…</div>
  <div v-else-if="!servers.length" class="panel"><div class="empty">Nobody's playing right now.</div></div>
  <template v-else>
    <section v-for="sv in servers" :key="sv.id" class="panel">
      <header><h2>{{ sv.name }}</h2><span class="small muted">{{ fmt.n(sv.online.length) }} online</span></header>
      <div v-if="sv.games.length" class="table-wrap">
        <table>
          <thead><tr><th>Game</th><th>Host</th><th>Players</th><th class="r">For</th><th></th></tr></thead>
          <tbody>
            <tr v-for="g in sv.games" :key="g.id">
              <td>
                <b>{{ g.room_kind === 'match' ? (where(data.labels, g) || 'A match') : 'Party' }}</b>
                <div v-if="g.room_kind === 'match'" class="small muted">{{ roomKind(g) }}</div>
              </td>
              <td>{{ g.host || '?' }}</td>
              <td>
                <span class="tags">
                  <span v-for="name in g.players" :key="name" class="pill" :class="playerTone(sv.id, name)" :title="playerTip(sv.id, name)">{{ name }}<template v-if="ping(sv.id, name) != null"> · {{ ping(sv.id, name) }} ms</template></span>
                </span>
              </td>
              <td class="r nowrap">{{ g.since ? dur(now - g.since) || '<1 s' : '–' }}</td>
              <td class="r"><RouterLink :to="matchLink(sv.id, g.id, g.since)" class="small">Timeline →</RouterLink></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty small">No rooms open.</div>
      <p v-if="sv.menus.length" class="small muted menus">In the menus: {{ sv.menus.map(p => p.name).join(', ') }}</p>
    </section>
  </template>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { ensureOverview } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { dur, matchLink, roomKind, where } from '../lib/stays.js';

const data = ref(null);
const error = ref('');
const now = ref(Math.floor(Date.now() / 1000));
let timer;
async function load() {
  try {
    data.value = await api('GET', '/games');
    error.value = '';
  } catch (e) {
    if (e.status !== 401) error.value = e.message;
  }
  now.value = Math.floor(Date.now() / 1000);
}
onMounted(() => {
  ensureOverview().catch(() => {});
  load();
  timer = setInterval(load, 10000);
});
onUnmounted(() => clearInterval(timer));

const games = computed(() => data.value?.games || []);
const players = computed(() => data.value?.players || []);
const matches = computed(() => games.value.filter(g => g.room_kind === 'match'));
const parties = computed(() => games.value.filter(g => g.room_kind !== 'match'));
const inMatches = computed(() => players.value.filter(p => p.status === 'match').length);
const relayed = computed(() => players.value.filter(p => p.network === 'relayed').length);
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));

const servers = computed(() => {
  const ids = [...new Set([...games.value.map(g => g.server), ...players.value.map(p => p.server)])];
  return ids
    .map(id => ({
      id,
      name: names.value.get(id) || id,
      // Matches first, the longest-running first; then parties.
      games: games.value
        .filter(g => g.server === id)
        .sort((a, b) => (a.room_kind === 'match' ? 0 : 1) - (b.room_kind === 'match' ? 0 : 1) || (a.since || 0) - (b.since || 0)),
      online: players.value.filter(p => p.server === id),
      menus: players.value.filter(p => p.server === id && p.status === 'menus'),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
});

const playerOf = (sv, name) => players.value.find(p => p.server === sv && p.name.toLowerCase() === name.toLowerCase());
const ping = (sv, name) => playerOf(sv, name)?.ping_ms ?? null;
function playerTone(sv, name) {
  const n = playerOf(sv, name)?.network;
  return n === 'unregistered' ? 'bad' : n === 'relayed' ? 'info' : '';
}
function playerTip(sv, name) {
  const p = playerOf(sv, name);
  if (!p) return 'Not in the online list (signed out?)';
  const net = { relayed: 'relayed', direct: 'direct', unregistered: 'not registered for online play: nobody can reach them' }[p.network] || '';
  return [[p.city, p.country_name || p.country].filter(Boolean).join(', '), net, p.since ? `on for ${dur(now.value - p.since)}` : ''].filter(Boolean).join(' · ');
}
</script>

<style scoped>
.nowrap { white-space: nowrap; }
.menus { margin: 10px 2px 0; }
</style>
