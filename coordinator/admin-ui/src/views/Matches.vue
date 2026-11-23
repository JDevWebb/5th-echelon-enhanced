<template>
  <PageTop title="Matches" sub="Every match the servers told of, newest first, kept for good. Open one to see what happened, its results and its recordings (kept 30 days).">
    <template v-if="data" #stats>
      <Stat :value="fmt.n(list.length)" label="Matches" :sub="data.next ? 'this page' : 'in the range'" />
      <Stat :value="fmt.n(list.filter(m => m.recordings).length)" label="With recordings" />
      <Stat :value="fmt.n(list.filter(m => m.results).length)" label="With results" />
    </template>
  </PageTop>
  <section class="panel">
    <header><h2>Find a match</h2></header>
    <form class="filters" @submit.prevent="search">
      <label>From <input v-model="f.from" type="date" /></label>
      <label>To <input v-model="f.to" type="date" /></label>
      <label>Server
        <select v-model="f.server">
          <option value="">Any</option>
          <option v-for="s in data?.servers || []" :key="s.id" :value="s.id">{{ s.name || s.id }}</option>
        </select>
      </label>
      <label>Mode
        <select v-model="f.mode">
          <option value="">Any</option>
          <option value="svm">Spies vs Mercs</option>
          <option value="coop">Co-op</option>
        </select>
      </label>
      <label>Map
        <select v-model="f.map">
          <option value="">Any</option>
          <option v-for="l in maps" :key="l.id" :value="String(l.id)">{{ l.name }}</option>
        </select>
      </label>
      <label>Player <input v-model.trim="f.player" placeholder="name or account" /></label>
      <label class="check"><input v-model="f.recordings" type="checkbox" /> Has recordings</label>
      <label class="check"><input v-model="f.results" type="checkbox" /> Has results</label>
      <button type="submit" class="btn">Search</button>
    </form>
  </section>
  <div v-if="error && !data" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else-if="!data" class="empty">Loading…</div>
  <section v-else class="panel">
    <div v-if="!list.length" class="empty">No matches with this choice.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>Started</th><th>Match</th><th>Server</th><th class="r">Lasted</th><th>Players</th><th class="r">Recordings</th><th class="r">Results</th></tr></thead>
        <tbody>
          <tr v-for="m in list" :key="`${m.server}-${m.room}-${m.since}`">
            <td class="nowrap small">{{ fmt.when(m.since) }}</td>
            <td>
              <RouterLink :to="`/match/${m.server}/${m.room}/${m.since}`"><b>{{ where(data.labels, m) || 'A match' }}</b></RouterLink>
              <div class="small muted">{{ roomKind({ ...m, kind: 'match' }) }}{{ m.host_name ? `, hosted by ${m.host_name}` : '' }}{{ m.under_way ? '' : ', never under way' }}</div>
            </td>
            <td class="small">{{ serverName(m.server) }}</td>
            <td class="r small nowrap">{{ fmt.dur((m.ended_at || m.last_at) - m.since) }}</td>
            <td class="small">{{ m.players.map(p => p.name || `#${p.player}`).join(', ') }}</td>
            <td class="r">{{ m.recordings ? fmt.n(m.recordings) : '–' }}</td>
            <td class="r">{{ m.results ? fmt.n(m.results) : 'none' }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-if="data.next" class="row" style="justify-content: center; margin-top: 12px">
      <button type="button" class="btn" @click="older">Older matches</button>
    </div>
  </section>
</template>

<script setup>
import { computed, reactive, ref } from 'vue';
import { RouterLink } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt } from '../lib/fmt.js';
import { roomKind, where } from '../lib/stays.js';

const day = t => new Date(t * 1000).toISOString().slice(0, 10);
const now = Math.floor(Date.now() / 1000);
const f = reactive({ from: day(now - 7 * 86400), to: day(now), server: '', mode: '', map: '', player: '', recordings: false, results: false });
const asked = ref({ ...f, before: null });

const { data, error } = useLoad(() => {
  const q = asked.value;
  const p = new URLSearchParams({
    from: String(Math.floor(new Date(`${q.from}T00:00:00Z`).getTime() / 1000)),
    to: String(Math.floor(new Date(`${q.to}T00:00:00Z`).getTime() / 1000) + 86400),
  });
  for (const k of ['server', 'mode', 'map', 'player']) if (q[k]) p.set(k, q[k]);
  if (q.recordings) p.set('recordings', 'true');
  if (q.results) p.set('results', 'true');
  if (q.before) p.set('before', String(q.before));
  return api('GET', `/matches?${p}`);
}, () => asked.value, { refresh: false });

const list = computed(() => data.value?.matches || []);
const maps = computed(() => (data.value?.labels || []).filter(l => l.kind === 'map').sort((a, b) => a.name.localeCompare(b.name)));
const serverName = id => (data.value?.servers || []).find(s => s.id === id)?.name || id;

function search() {
  asked.value = { ...f, before: null };
}
function older() {
  asked.value = { ...asked.value, before: data.value.next };
}
</script>

<style scoped>
.filters { display: flex; flex-wrap: wrap; gap: 10px 14px; align-items: end; }
.filters label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; }
.filters label.check { flex-direction: row; align-items: center; }
</style>
