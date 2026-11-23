<template>
  <section class="panel">
    <header class="top">
      <h2>{{ player ? `${player.name || short(player.identity)}'s chat` : conv ? convTitle(conv) : 'Every message' }}</h2>
      <button v-if="player || conv" class="small" type="button" @click="clearAll">Show everyone</button>
    </header>
    <form class="filters row" @submit.prevent="search">
      <div class="seg" role="group" aria-label="Room">
        <button v-for="[id, label] in ROOMS" :key="id" type="button" :aria-pressed="String(room === id)" :disabled="!!conv" @click="room = id">{{ label }}</button>
      </div>
      <input v-model="text" class="q" type="search" maxlength="100" placeholder="Text or sender's name" aria-label="Search" />
      <button class="small" type="submit">Search</button>
      <span class="small muted">Every message players sent: Global, lobbies and direct messages, deleted ones too. Reading a player's chat or a direct conversation goes to the audit log.</span>
    </form>

    <div v-if="player" class="who small">
      <template v-for="(a, i) in player.detail.accounts" :key="a.server + a.id"><template v-if="i"> · </template><RouterLink :to="`/players/${a.server}/${a.id}`">{{ a.name }} on {{ names.get(a.server) || a.server }}</RouterLink></template>
      <span v-for="mu in player.detail.mutes" :key="mu.room" class="pill warn">Muted {{ mu.room === '*' ? 'everywhere' : 'in ' + (ROOM[mu.room] || mu.room) }}</span>
    </div>
    <div v-if="conversations?.length && !conv" class="convs">
      <span class="small muted">Their conversations:</span>
      <button v-for="cv in conversations" :key="cv.conv" class="small" type="button" :title="`${cv.count} messages, the last ${fmt.ago(cv.last)}`" @click="openConv(cv.conv)">
        {{ cv.room === 'direct' ? `With ${cv.with_name || short(cv.with)}` : convTitle(cv.conv) }} <span class="muted">{{ fmt.n(cv.count) }}</span>
      </button>
    </div>

    <div v-if="error" class="empty">{{ error }}</div>
    <div v-else-if="!messages.length" class="empty">{{ loading ? 'Loading…' : 'No messages.' }}</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Where</th><th>From</th><th>Message</th></tr></thead>
        <tbody>
          <tr v-for="m in messages" :key="m.id" :class="{ gone: m.deleted_at }">
            <td class="nowrap" :title="fmt.when(m.at)">{{ fmt.when(m.at) }}</td>
            <td class="nowrap"><button class="link" type="button" :title="m.conv" @click="openConv(m.conv)">{{ where(m) }}</button></td>
            <td class="nowrap"><button class="link" type="button" :title="m.from" @click="openPlayer(m.from)">{{ m.from_name }}</button></td>
            <td class="body">
              <span class="what">{{ m.body }}</span>
              <span v-if="m.deleted_at" class="pill">Deleted by {{ m.deleted_by === 'sender' ? 'the sender' : m.deleted_by.slice(6) }}</span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-if="more" class="row"><button class="small" type="button" :disabled="loading" @click="load(true)">Older</button></div>
  </section>
</template>

<script setup>
// The whole chat for the admins: every room, a player's (sent and received, with their
// conversations), one conversation, or text. Newest first, 200 a page.
import { computed, onMounted, ref, watch } from 'vue';
import { RouterLink, useRoute, useRouter } from 'vue-router';
import { api } from '../lib/api.js';
import { ensureOverview } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const ROOM = { global: 'Global', direct: 'Direct', lobby: 'Lobby' };
const ROOMS = [['all', 'All'], ['global', 'Global'], ['lobby', 'Lobbies'], ['direct', 'Direct']];
const short = id => (id ? `${id.slice(0, 4)}-${id.slice(-4)}` : '');

const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));

const room = ref('all');
const text = ref('');
const playerId = computed(() => (typeof route.query.player === 'string' ? route.query.player : ''));
const conv = computed(() => (typeof route.query.conv === 'string' ? route.query.conv : ''));
const messages = ref([]);
const conversations = ref(null);
const player = ref(null);
const more = ref(false);
const loading = ref(false);
const error = ref('');

function convTitle(c) {
  if (c === 'global') return 'Global';
  if (c.startsWith('lobby:')) {
    const [, server, session] = c.split(':');
    return `Lobby ${session} on ${names.value.get(server) || server}`;
  }
  return 'A direct conversation';
}
const where = m => (m.room === 'direct' ? `Direct to ${m.to_name || short(m.to) || '?'}` : convTitle(m.conv));

async function load(older = false) {
  loading.value = true;
  error.value = '';
  const q = new URLSearchParams();
  if (!conv.value && room.value !== 'all') q.set('room', room.value);
  if (playerId.value) q.set('player', playerId.value);
  if (conv.value) q.set('conv', conv.value);
  if (text.value.trim()) q.set('q', text.value.trim());
  if (older && messages.value.length) q.set('before', messages.value[messages.value.length - 1].id);
  try {
    const v = await api('GET', `/moderation/chat?${q}`);
    messages.value = older ? [...messages.value, ...v.messages] : v.messages;
    more.value = v.more;
    if (!older) {
      conversations.value = v.conversations || null;
      player.value = v.player || null;
    }
  } catch (e) {
    error.value = e.message;
  } finally {
    loading.value = false;
  }
}

const search = () => load();
const openPlayer = id => router.replace({ query: { ...route.query, tab: 'transcript', player: id, conv: undefined } });
const openConv = c => router.replace({ query: { ...route.query, tab: 'transcript', conv: c } });
const clearAll = () => router.replace({ query: { tab: 'transcript' } });
watch([room, playerId, conv], () => load(), { immediate: true });
</script>

<style scoped>
.top { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.filters { gap: 8px; flex-wrap: wrap; align-items: center; margin-bottom: 10px; }
.q { flex: 1 1 200px; min-width: 0; }
.who, .convs { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin-bottom: 10px; }
.body { word-break: break-word; white-space: pre-wrap; }
.nowrap { white-space: nowrap; }
.body .pill { display: inline-block; margin-left: 6px; }
.gone .what { text-decoration: line-through; color: var(--muted); }
.link { background: none; border: 0; padding: 0; color: var(--accent); cursor: pointer; font: inherit; text-align: left; }
</style>
