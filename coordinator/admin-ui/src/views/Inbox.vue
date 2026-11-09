<template>
  <PageTop title="Inbox" sub="Everything waiting on an admin: support conversations, player reports, reported chat, new crashes and open alerts. Deal with them here or on their own pages.">
    <template #stats>
      <Stat :value="data ? fmt.n(data.counts.all) : '–'" label="Waiting" :tone="data?.counts.all ? 'warn' : ''" />
      <Stat :value="data ? fmt.n(data.counts.support) : '–'" label="Support" />
      <Stat :value="data ? fmt.n(data.counts.report + data.counts.chat) : '–'" label="Reports" />
      <Stat :value="data ? fmt.n(data.counts.crash) : '–'" label="Crashes" />
      <Stat :value="data ? fmt.n(data.counts.alert) : '–'" label="Alerts" :tone="data?.counts.alert ? 'bad' : ''" />
    </template>
  </PageTop>
  <div class="grid split">
    <section class="panel list">
      <header class="filters">
        <div class="seg" role="group" aria-label="Show">
          <button v-for="[id, label] in KINDS" :key="id" type="button" :aria-pressed="String(kind === id)" @click="kind = id">
            {{ label }}<span v-if="data && data.counts[id]" class="muted"> {{ data.counts[id] }}</span>
          </button>
        </div>
        <select v-model="server" aria-label="Server">
          <option value="">All servers</option>
          <option v-for="sv in servers" :key="sv.id" :value="sv.id">{{ serverName(sv) }}</option>
        </select>
      </header>
      <div v-if="!data" class="empty">{{ error || 'Loading…' }}</div>
      <div v-else-if="!shown.length" class="empty">{{ data.counts.all ? 'Nothing of that kind.' : 'Nothing waiting on you.' }}</div>
      <ul v-else class="items" aria-label="Waiting">
        <li v-for="(i, n) in shown" :key="id(i)">
          <button :ref="el => { if (el) rows[n] = el; }" type="button" class="item" :class="{ picked: id(i) === openKey }" @click="pick(i)">
            <span class="kind" :class="i.kind" :title="LABEL[i.kind]" aria-hidden="true"><svg viewBox="0 0 24 24" v-html="ICON[i.kind]"></svg></span>
            <span class="body">
              <span class="line">
                <b class="who" :class="{ mono: i.kind === 'crash' }">{{ i.who || 'someone' }}</b>
                <span class="muted small nowrap" :title="fmt.when(i.at)">{{ fmt.ago(i.at) }}</span>
              </span>
              <span class="line">
                <span class="title">{{ i.title || '–' }}</span>
                <span class="tags">
                  <span v-if="i.server" class="muted small nowrap">{{ names.get(i.server) || i.server }}</span>
                  <span class="pill" :class="tone(i)">{{ pillText(i) }}</span>
                </span>
              </span>
            </span>
          </button>
        </li>
      </ul>
      <p class="small muted keys"><kbd>j</kbd> <kbd>k</kbd> next and previous · <kbd>o</kbd> open on its own page</p>
    </section>

    <SupportThread v-if="open?.kind === 'support'" :key="open.key" :identity="open.key" @changed="reload" @gone="dropped" />
    <ReportDetail v-else-if="open?.kind === 'report'" :id="open.key" :key="open.key" :names="names" @close="close" @changed="reload" />
    <section v-else-if="open?.kind === 'chat'" class="panel">
      <header>
        <div>
          <h2>{{ open.who }} <span class="muted small">in {{ open.room }}</span></h2>
          <p class="small muted">{{ names.get(open.server) || open.server }} · reported {{ open.reports === 1 ? 'once' : `${open.reports} times` }} · {{ fmt.ago(open.at) }}</p>
        </div>
        <button class="ghost small" type="button" aria-label="Close" @click="close">✕</button>
      </header>
      <blockquote class="said">{{ open.title }}</blockquote>
      <label class="field"><span>Note <span class="muted">(admins only)</span></span><textarea v-model="note" maxlength="500" rows="2" placeholder="What it was, what was done"></textarea></label>
      <div class="row end">
        <RouterLink class="btn small ghost" to="/moderation">Mute or more on Chat and names</RouterLink>
        <button class="small" type="button" :disabled="busy" @click="deal('dismiss')">Dismiss</button>
        <button class="small danger" type="button" :disabled="busy" @click="deal('delete')">Delete message</button>
        <button class="small primary" type="button" :disabled="busy" @click="deal('resolve')">Dealt with</button>
      </div>
    </section>
    <section v-else-if="open?.kind === 'crash'" class="panel">
      <header>
        <div>
          <h2 class="mono">{{ open.who }}</h2>
          <p class="small muted">Last {{ fmt.ago(open.at) }} · {{ open.status === 'back' ? 'marked fixed, and seen again since' : 'new: not marked yet' }}</p>
        </div>
        <button class="ghost small" type="button" aria-label="Close" @click="close">✕</button>
      </header>
      <p>{{ open.title }} in the last week.</p>
      <p class="small muted">Its registers, code, callers, what the game was doing and the reports the same players sent are on the Crashes page, where it can be marked fixed, known or watching.</p>
      <div class="row end"><RouterLink class="btn small primary" :to="{ path: '/crashes', query: { place: open.key } }">Open on Crashes</RouterLink></div>
    </section>
    <section v-else-if="open?.kind === 'alert'" class="panel">
      <header>
        <div>
          <h2>{{ open.who }}</h2>
          <p class="small muted">{{ open.server ? names.get(open.server) || open.server : 'The network' }} · since {{ fmt.ago(open.at) }}</p>
        </div>
        <button class="ghost small" type="button" aria-label="Close" @click="close">✕</button>
      </header>
      <p><span class="pill" :class="open.status">{{ open.status }}</span> {{ open.title }}</p>
      <p class="small muted">It closes on its own when the condition clears.</p>
      <div class="row end"><RouterLink class="btn small primary" to="/alerts">Open Alerts</RouterLink></div>
    </section>
    <section v-else class="panel"><div class="empty">{{ data?.counts.all ? 'Pick something from the list.' : 'All clear.' }}</div></section>
  </div>
</template>

<script setup>
// Everything waiting on an admin (GET /api/inbox), newest first; picking one deals with it
// beside the list: a support conversation or a player report in full, a reported chat message
// with its actions, a crash or an alert with a link to its page. #/inbox?open=<kind>:<key>.
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import ReportDetail from '../components/ReportDetail.vue';
import Stat from '../components/Stat.vue';
import SupportThread from '../components/SupportThread.vue';
import { api } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const KINDS = [['all', 'All'], ['support', 'Support'], ['report', 'Reports'], ['chat', 'Chat'], ['crash', 'Crashes'], ['alert', 'Alerts']];
const LABEL = { support: 'Support', report: 'Player report', chat: 'Reported chat', crash: 'Crash', alert: 'Alert' };
const ICON = {
  support: '<path d="M4 5h16v11H8l-4 4z" />',
  report: '<path d="M5 21V4h11l-1.5 4L16 12H5" />',
  chat: '<path d="M4 6h16M4 12h10M4 18h7" />',
  crash: '<path d="M12 3 2 20h20zM12 10v4M12 17v.5" />',
  alert: '<path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4zM10 21h4" />',
};

const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const servers = computed(() => live.overview?.servers || []);
const names = computed(() => new Map(servers.value.map(sv => [sv.id, serverName(sv)])));

const { data, error, reload } = useLoad(() => api('GET', '/inbox'), () => '', { refresh: false });
watch(() => [live.supportTick, live.reportTick, live.moderationTick, live.alertTick], () => reload());

const kind = ref('all');
const server = ref('');
const id = i => `${i.kind}:${i.key}`;
const shown = computed(() => (data.value?.items || []).filter(i => (kind.value === 'all' || i.kind === kind.value) && (!server.value || i.server === server.value)));

// The picked one, by its address (it stays open while the list reloads, until it's dealt with).
const openKey = computed(() => (typeof route.query.open === 'string' ? route.query.open : ''));
const kept = ref(null);
const open = computed(() => (data.value?.items || []).find(i => id(i) === openKey.value) || (kept.value && id(kept.value) === openKey.value ? kept.value : null));
watch(open, i => { if (i) kept.value = i; });
const pick = i => router.replace({ path: '/inbox', query: { open: id(i) } });
const close = () => router.replace({ path: '/inbox' });
function dropped() {
  kept.value = null;
  close();
  reload();
}

const tone = i => ({ support: 'warn', report: 'warn', chat: 'warn', crash: i.status === 'back' ? 'bad' : 'info', alert: i.status }[i.kind] || '');
function pillText(i) {
  if (i.kind === 'support') return i.unread ? `${i.unread} new` : 'open';
  if (i.kind === 'chat') return `${i.reports} report${i.reports === 1 ? '' : 's'}`;
  if (i.kind === 'crash') return i.status === 'back' ? 'back since fix' : `${i.players} player${i.players === 1 ? '' : 's'}`;
  if (i.kind === 'alert') return i.status;
  return i.rating || 'open';
}

// A reported chat message, dealt with here.
const note = ref('');
const busy = ref(false);
watch(openKey, () => { note.value = ''; });
async function deal(action) {
  const i = open.value;
  if (action === 'delete' && !await confirmBox('Delete this message?', `${i.who}'s message goes for everyone who can see it. The report is kept 90 days.`, 'Delete', true)) return;
  busy.value = true;
  try {
    await api('POST', `/moderation/reports/${i.key}`, { action, note: note.value });
    toast({ delete: 'Deleted.', resolve: 'Marked dealt with.', dismiss: 'Dismissed.' }[action]);
    dropped();
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}

// j and k step through the list, o opens the picked one on its own page.
const rows = [];
function own(i) {
  return {
    support: `/support/${i.key}`, report: '/reports', chat: '/moderation', crash: { path: '/crashes', query: { place: i.key } }, alert: '/alerts',
  }[i.kind];
}
function keys(e) {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const t = e.target;
  if (t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName))) return;
  const list = shown.value;
  const at = list.findIndex(i => id(i) === openKey.value);
  if (e.key === 'j' || e.key === 'k') {
    if (!list.length) return;
    const next = e.key === 'j' ? Math.min(list.length - 1, at + 1) : Math.max(0, at < 0 ? 0 : at - 1);
    pick(list[next]);
    rows[next]?.scrollIntoView({ block: 'nearest' });
    e.preventDefault();
  } else if (e.key === 'o' && at >= 0) {
    router.push(own(list[at]));
    e.preventDefault();
  }
}
onMounted(() => window.addEventListener('keydown', keys));
onUnmounted(() => window.removeEventListener('keydown', keys));
</script>

<style scoped>
.split { grid-template-columns: minmax(320px, 0.9fr) minmax(0, 1.3fr); align-items: start; }
.filters { flex-wrap: wrap; gap: 8px; }
.filters select { width: auto; }
.items { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; max-height: 70vh; overflow-y: auto; }
.item {
  width: 100%; display: flex; gap: 10px; align-items: flex-start; text-align: left; padding: 10px 8px;
  border: none; border-bottom: 1px solid var(--line-soft); border-radius: 0; background: none; color: var(--text); font: inherit; cursor: pointer;
}
.item:hover { background: var(--panel-2); }
.item.picked { background: var(--accent-soft); box-shadow: inset 3px 0 0 var(--accent); }
.kind { flex: none; width: 28px; height: 28px; border-radius: 7px; display: grid; place-items: center; background: var(--chip); color: var(--muted); }
.kind svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
.kind.support { color: var(--info); }
.kind.report, .kind.chat { color: var(--warn); }
.kind.crash, .kind.alert { color: var(--bad); }
.body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.line { display: flex; justify-content: space-between; align-items: center; gap: 8px; min-width: 0; }
.who { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.title { color: var(--soft); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.tags { display: flex; gap: 6px; align-items: center; flex: none; }
.nowrap { white-space: nowrap; }
.keys { margin: 10px 0 0; }
.said { margin: 0 0 12px; padding: 10px 14px; border-left: 3px solid var(--warn); background: var(--panel-2); border-radius: 0 8px 8px 0; white-space: pre-wrap; word-break: break-word; }
.end { justify-content: flex-end; flex-wrap: wrap; margin-top: 10px; }
@media (max-width: 1100px) { .split { grid-template-columns: 1fr; } }
</style>
