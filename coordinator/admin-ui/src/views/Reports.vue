<template>
  <PageTop title="Reports" sub="What players said after their sessions, with their logs (redacted on their PC) and their server's.">
    <template #stats>
      <Stat :value="fmt.n(live.overview?.open_reports || 0)" label="Open" :tone="live.overview?.open_reports ? 'info' : ''" />
      <Stat :value="data ? fmt.n(data.total) : '–'" :label="`${STATUSES.find(([id]) => id === status)[1]}${q || server || problem ? ', matching' : ''}`" />
    </template>
    <div class="seg" role="group" aria-label="Status">
      <button v-for="[id, label] in STATUSES" :key="id" type="button" :aria-pressed="String(status === id)" @click="status = id">{{ label }}</button>
    </div>
  </PageTop>
  <div class="grid" :class="{ split: openId }">
    <section class="panel">
      <header>
        <h2>{{ STATUSES.find(([id]) => id === status)[1] }} <span class="muted small">{{ data ? fmt.n(data.total) : '' }}</span></h2>
        <form class="row filters" role="search" @submit.prevent>
          <input v-model="q" type="search" placeholder="Player, comment, identity or report id" aria-label="Search reports">
          <select v-model="server" aria-label="Server">
            <option value="">All servers</option>
            <option v-for="sv in servers" :key="sv.id" :value="sv.id">{{ serverName(sv) }}</option>
          </select>
          <select v-model="problem" aria-label="Problem">
            <option value="">Any problem</option>
            <option v-for="(label, id) in PROBLEMS" :key="id" :value="id">{{ label }}</option>
          </select>
        </form>
      </header>
      <div v-if="!data" class="empty">{{ error || 'Loading…' }}</div>
      <div v-else-if="!data.reports.length" class="empty">{{ q || server || problem ? 'No reports match.' : status === 'open' ? 'No open reports.' : 'No reports yet. Launchers ask players after a session with problems, and now and then after a good one.' }}</div>
      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr><th>When</th><th>Player</th><th>Server</th><th>Rating</th><th>Problems</th><th>Triggers</th><th>Comment</th><th class="r">Files</th></tr>
          </thead>
          <tbody>
            <tr v-for="r in data.reports" :key="r.id" :class="{ picked: openId === r.id }" tabindex="0" @click="pick(r.id)" @keydown.enter="pick(r.id)">
              <td class="muted nowrap" :title="fmt.when(r.created_at)">{{ fmt.ago(r.created_at) }}</td>
              <td><b>{{ r.player.name }}</b><span v-if="r.status === 'resolved'" class="pill ok tiny">resolved</span></td>
              <td class="muted">{{ names.get(r.server) || r.server }}</td>
              <td><RatingPill :rating="r.rating" /></td>
              <td><span class="tags"><span v-for="p in r.problems" :key="p" class="pill warn">{{ PROBLEMS[p] || p }}</span></span></td>
              <td class="small muted">{{ r.triggers.join(', ') || '–' }}</td>
              <td class="comment">{{ r.comment || '–' }}{{ r.comment_cut ? '…' : '' }}</td>
              <td class="r muted">{{ r.files.length || '–' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-if="data && data.total > data.per_page" class="row pager">
        <span class="small muted">{{ fmt.n(data.page * data.per_page + 1) }}–{{ fmt.n(Math.min(data.total, (data.page + 1) * data.per_page)) }} of {{ fmt.n(data.total) }}</span>
        <span class="row">
          <button class="small" type="button" :disabled="page === 0" @click="page--">Previous</button>
          <button class="small" type="button" :disabled="(page + 1) * data.per_page >= data.total" @click="page++">Next</button>
        </span>
      </div>
    </section>
    <ReportDetail v-if="openId" :id="openId" :names="names" @close="close" @changed="reload" @open="pick" />
  </div>
</template>

<script setup>
// Players' reports, newest first; picking one opens it beside the list (#/reports/<id>).
import { computed, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import RatingPill from '../components/RatingPill.vue';
import Stat from '../components/Stat.vue';
import ReportDetail from '../components/ReportDetail.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, PROBLEMS, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const STATUSES = [['open', 'Open'], ['resolved', 'Resolved'], ['all', 'All']];
const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));
const status = ref('open');
const server = ref('');
const problem = ref('');
const q = ref('');
const page = ref(0);
const openId = computed(() => (/^[0-9a-f]{32}$/i.test(route.params.id || '') ? route.params.id.toLowerCase() : null));

// Typing waits a moment before searching.
const search = ref('');
let typing;
watch(q, v => { clearTimeout(typing); typing = setTimeout(() => { search.value = v.trim(); }, 250); });
watch([search, server, problem, status], () => { page.value = 0; });

const query = computed(() => new URLSearchParams({ status: status.value, server: server.value, problem: problem.value, q: search.value, page: String(page.value) }).toString());
const { data, error, reload } = useLoad(() => api('GET', `/reports?${query.value}`), () => query.value, { refresh: false });
watch(() => live.reportTick, reload);
const pick = id => router.push(`/reports/${id}`);
const close = () => router.push('/reports');
</script>

<style scoped>
.split { grid-template-columns: minmax(0, 1.2fr) minmax(400px, 1fr); }
.filters { flex-wrap: wrap; }
.filters input[type="search"] { width: 260px; }
.filters select { width: auto; }
tbody tr { cursor: pointer; }
tbody tr.picked td { background: var(--accent-soft); }
.nowrap { white-space: nowrap; }
.tags { display: inline-flex; flex-wrap: wrap; gap: 4px; }
.tiny { margin-left: 6px; font-size: 11px; padding: 1px 7px; }
.comment { max-width: 280px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--soft); }
.pager { justify-content: space-between; margin-top: 10px; }
@media (max-width: 1100px) { .split { grid-template-columns: 1fr; } }
</style>
