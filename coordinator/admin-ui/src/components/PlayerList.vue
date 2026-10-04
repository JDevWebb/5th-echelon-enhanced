<template>
  <div class="grid" :class="{ split: open }">
    <section class="panel">
      <header>
        <h2>Players <span class="muted small">{{ data ? fmt.n(data.total) : '' }}</span></h2>
        <form class="row filters" role="search" @submit.prevent>
          <input v-model="q" type="search" placeholder="Name, account # or identity" aria-label="Search players">
          <select v-model="server" aria-label="Server">
            <option value="">All servers</option>
            <option v-for="sv in servers" :key="sv.id" :value="sv.id">{{ serverName(sv) }}</option>
          </select>
          <label class="row check"><input v-model="online" type="checkbox" style="width: auto"><span>Online</span></label>
          <label class="row check"><input v-model="banned" type="checkbox" style="width: auto"><span>Banned</span></label>
        </form>
      </header>
      <div v-if="!data" class="empty">{{ error || 'Loading…' }}</div>
      <div v-else-if="!data.players.length" class="empty">{{ q || server || online || banned ? 'No players match.' : 'No players yet. Servers send theirs when they start (newer releases only).' }}</div>
      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr>
              <th><button class="sort" type="button" :aria-pressed="String(sort === 'name')" @click="sort = 'name'">Name</button></th>
              <th>Server</th>
              <th><button class="sort" type="button" :aria-pressed="String(sort === 'last_seen')" @click="sort = 'last_seen'">Last seen</button></th>
              <th class="r"><button class="sort" type="button" :aria-pressed="String(sort === 'play_time')" @click="sort = 'play_time'">Played</button></th>
              <th class="r">7 days</th>
              <th class="r">Sessions</th>
              <th class="r">Matches</th>
              <th><button class="sort" type="button" :aria-pressed="String(sort === 'created')" @click="sort = 'created'">Joined</button></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in data.players" :key="p.server + p.id" :class="{ picked: open && open.server === p.server && open.id === p.id }" tabindex="0" @click="pick(p)" @keydown.enter="pick(p)">
              <td>
                <span class="row" style="gap: 8px">
                  <span class="dot" :class="{ ok: p.online }" :title="p.online ? 'online' : 'offline'"></span>
                  <b>{{ p.name }}</b>
                  <span v-if="p.banned_accounts" class="pill bad">{{ p.accounts > 1 ? `banned on ${p.banned_accounts} of ${p.accounts}` : 'banned' }}</span>
                </span>
              </td>
              <td class="muted">{{ p.servers.map(s => names.get(s) || s).join(', ') }}</td>
              <td class="muted">{{ p.online ? 'now' : fmt.ago(p.last_seen) }}</td>
              <td class="r">{{ fmt.dur(p.play_seconds) }}</td>
              <td class="r">{{ fmt.dur(p.week_seconds) }}</td>
              <td class="r">{{ fmt.n(p.sessions) }}</td>
              <td class="r">{{ fmt.n(p.matches) }}</td>
              <td class="muted">{{ p.created_at ? new Date(p.created_at * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' }) : '–' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-if="data && data.total > data.per_page" class="row" style="justify-content: space-between; margin-top: 10px">
        <span class="small muted">{{ fmt.n(data.page * data.per_page + 1) }}–{{ fmt.n(Math.min(data.total, (data.page + 1) * data.per_page)) }} of {{ fmt.n(data.total) }}</span>
        <span class="row">
          <button class="small" type="button" :disabled="page === 0" @click="page--">Previous</button>
          <button class="small" type="button" :disabled="(page + 1) * data.per_page >= data.total" @click="page++">Next</button>
        </span>
      </div>
    </section>
    <PlayerDetail v-if="open" :server="open.server" :id="open.id" :names="names" @close="close" @changed="reload" @open="pick" />
  </div>
</template>

<script setup>
// Every server's players, searchable; picking one opens their detail beside the list.
import { computed, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PlayerDetail from './PlayerDetail.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
const q = ref('');
const server = ref('');
const online = ref(false);
const banned = ref(false);
const sort = ref('last_seen');
const page = ref(0);
const open = ref(null);
// A player's own address (#/players/<server>/<id>, e.g. from a report) opens their detail.
const route = useRoute();
const router = useRouter();
watch(() => [route.params.server, route.params.id], ([sv, id]) => {
  if (sv && id && /^\d+$/.test(id)) open.value = { server: sv, id: Number(id) };
}, { immediate: true });

// Typing waits a moment before searching.
const search = ref('');
let typing;
watch(q, v => { clearTimeout(typing); typing = setTimeout(() => { search.value = v.trim(); }, 250); });
watch([search, server, online, banned, sort], () => { page.value = 0; });

const query = computed(() => new URLSearchParams({
  q: search.value, server: server.value, online: online.value ? '1' : '', banned: banned.value ? '1' : '', sort: sort.value, page: String(page.value),
}).toString());
const { data, error, reload } = useLoad(() => api('GET', `/players?${query.value}`), () => query.value);
const pick = p => {
  open.value = { server: p.server, id: p.id };
  router.replace({ path: `/players/${encodeURIComponent(p.server)}/${p.id}`, query: route.query });
};
const close = () => {
  open.value = null;
  router.replace({ path: '/players', query: route.query });
};
</script>

<style scoped>
.split { grid-template-columns: minmax(0, 1.35fr) minmax(360px, 1fr); }
.filters { flex-wrap: wrap; }
.filters input[type="search"] { width: 240px; }
.filters select { width: auto; }
.check { gap: 6px; font-size: 13px; color: var(--muted); }
.sort { background: none; border: none; padding: 0; font: inherit; color: inherit; text-transform: inherit; letter-spacing: inherit; cursor: pointer; }
.sort[aria-pressed="true"] { color: var(--accent); }
.sort[aria-pressed="true"]::after { content: " ↓"; }
.sort:not([aria-pressed="true"]):hover { color: var(--text); }
tbody tr { cursor: pointer; }
tbody tr.picked td { background: var(--accent-soft); }
@media (max-width: 1100px) { .split { grid-template-columns: 1fr; } }
</style>
