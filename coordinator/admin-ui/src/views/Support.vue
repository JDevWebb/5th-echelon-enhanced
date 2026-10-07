<template>
  <PageTop title="Support" sub="Players write from the launcher's Support page, with their logs if they choose. Answer here: they read it in the launcher, and the game's overlay tells them.">
    <template #stats>
      <Stat :value="fmt.n(live.overview?.support_unread || 0)" label="Unread" :tone="live.overview?.support_unread ? 'info' : ''" />
      <Stat :value="list ? fmt.n(list.threads.length) : '–'" :label="FILTERS.find(([id]) => id === filter)[1]" />
    </template>
    <div class="seg" role="group" aria-label="Show">
      <button v-for="[id, label] in FILTERS" :key="id" type="button" :aria-pressed="String(filter === id)" @click="filter = id">{{ label }}</button>
    </div>
  </PageTop>
  <div class="grid split">
    <section class="panel">
      <header><h2>Conversations</h2></header>
      <div v-if="!list" class="empty">{{ listError || 'Loading…' }}</div>
      <div v-else-if="!list.threads.length" class="empty">{{ filter === 'active' ? 'Nothing waiting. Players write from the launcher\'s Support page.' : 'None.' }}</div>
      <ul v-else class="threads">
        <li v-for="t in list.threads" :key="t.identity">
          <button type="button" class="thread" :class="{ picked: openId === t.identity, unread: t.unread }" @click="pick(t.identity)">
            <span class="row between">
              <b>{{ t.name }}</b>
              <span class="muted small nowrap" :title="fmt.when(t.updated_at)">{{ fmt.ago(t.updated_at) }}</span>
            </span>
            <span class="row between">
              <span class="last">{{ t.last_from_admin ? 'You: ' : '' }}{{ t.last }}</span>
              <span class="row tight">
                <span v-if="t.unread" class="pill info">{{ t.unread }} new</span>
                <span class="pill" :class="STATUS_TONE[t.status]">{{ STATUS_LABEL[t.status] }}</span>
              </span>
            </span>
          </button>
        </li>
      </ul>
    </section>

    <section v-if="openId" class="panel conversation">
      <!-- No conversation yet: an admin writes first (from the player's page). -->
      <template v-if="!thread && fresh">
        <header>
          <div>
            <h2>{{ fresh }} <span class="muted small mono" :title="openId">{{ openId.slice(0, 4) }}-{{ openId.slice(-4) }}</span></h2>
            <p class="small muted">No conversation yet. What you write starts one: they read it on their launcher's Support page (0.4.3 and later), and the game's overlay tells them.</p>
          </div>
        </header>
      </template>
      <div v-else-if="!thread" class="empty">{{ threadError || 'Loading…' }}</div>
      <template v-else>
        <header>
          <div>
            <h2>{{ thread.name }} <span class="muted small mono" :title="thread.identity">{{ thread.short }}</span></h2>
            <p class="small muted">
              {{ thread.server || 'no server named' }}<template v-if="thread.launcher"> · launcher {{ thread.launcher }}</template>
              <template v-for="a in thread.accounts" :key="a.server + a.id"> · <RouterLink :to="`/players/${a.server}/${a.id}`">{{ a.name }} on {{ names.get(a.server) || a.server }}</RouterLink></template>
            </p>
          </div>
          <div class="thread-actions">
            <div class="seg" role="group" aria-label="Status">
              <button v-for="s in ['open', 'waiting', 'resolved']" :key="s" type="button" :aria-pressed="String(thread.status === s)" :disabled="busy" @click="setStatus(s)">{{ STATUS_LABEL[s] }}</button>
            </div>
            <button class="small danger" type="button" :disabled="busy" @click="remove">Delete</button>
          </div>
        </header>
        <div ref="scroller" class="messages">
          <div v-for="m in thread.messages" :key="m.id" class="msg" :class="m.from">
            <div class="meta small">
              <b>{{ m.from === 'admin' ? m.admin : thread.name }}</b>
              <span class="muted" :title="fmt.when(m.at)">{{ fmt.ago(m.at) }}</span>
            </div>
            <p class="text">{{ m.text }}</p>
            <div v-if="m.files.length" class="files">
              <a v-for="f in m.files" :key="f.name" class="btn small" :href="fileUrl(m.id, f.name)" :download="`support-${m.id}-${f.name}`">{{ f.name }} <span class="muted">{{ fmt.bytes(f.size) }}</span></a>
            </div>
          </div>
        </div>
      </template>
      <template v-if="thread || fresh">
        <form class="answer" @submit.prevent="answer">
          <label class="field">
            <span>{{ thread ? 'Answer' : 'Message' }} ({{ text.length }}/2000)</span>
            <textarea v-model="text" maxlength="2000" rows="4" placeholder="They read it in the launcher; the game's overlay tells them an answer came."></textarea>
          </label>
          <div class="row">
            <button class="primary" type="submit" :disabled="busy || !text.trim()">Send</button>
            <span class="small muted">Signed with your name, {{ me }}. Sending marks it waiting on the player.</span>
          </div>
        </form>
      </template>
    </section>
    <section v-else class="panel"><div class="empty">Pick a conversation.</div></section>
  </div>
</template>

<script setup>
// Players' conversations with the admins (newest first); picking one opens it beside the list
// (#/support/<identity>). The coordinator keeps them; the player's launcher reads the answers.
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const FILTERS = [['active', 'Active'], ['open', 'Their turn: open'], ['waiting', 'Waiting on them'], ['resolved', 'Resolved'], ['all', 'All']];
const STATUS_LABEL = { open: 'Open', waiting: 'Waiting', resolved: 'Resolved' };
const STATUS_TONE = { open: 'warn', waiting: 'info', resolved: 'ok' };

const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));
const filter = ref('active');
const openId = computed(() => (/^[A-Z2-7]{52}$/.test(route.params.identity || '') ? route.params.identity : null));
const me = ref('');
onMounted(async () => { try { me.value = (await api('GET', '/me')).username || ''; } catch { /* named on send */ } });

const { data: list, error: listError, reload: reloadList } = useLoad(() => api('GET', `/support?status=${filter.value}`), () => filter.value, { refresh: false });

const thread = ref(null);
const threadError = ref('');
// Starting a conversation: no thread yet, and the player's name from their page's link.
const fresh = ref('');
const text = ref('');
const busy = ref(false);
const scroller = ref(null);
async function loadThread() {
  if (!openId.value) { thread.value = null; return; }
  try {
    thread.value = await api('GET', `/support/${openId.value}`);
    threadError.value = '';
    fresh.value = '';
    await nextTick();
    if (scroller.value) scroller.value.scrollTop = scroller.value.scrollHeight;
  } catch (e) {
    const name = typeof route.query.name === 'string' ? route.query.name.slice(0, 40) : '';
    if (e.status === 404 && name) fresh.value = name;
    else threadError.value = e.message;
  }
}
watch(openId, () => { thread.value = null; text.value = ''; loadThread(); }, { immediate: true });
watch(() => live.supportTick, () => { reloadList(); loadThread(); });

const pick = id => router.push(`/support/${id}`);
const fileUrl = (message, name) => `/api/support/${openId.value}/files/${message}/${encodeURIComponent(name)}`;

async function answer() {
  if (!text.value.trim()) return;
  busy.value = true;
  try {
    thread.value = await api('POST', `/support/${openId.value}`, { text: text.value });
    text.value = '';
    toast('Sent. They\'ll see it in the launcher.');
    reloadList();
    await nextTick();
    if (scroller.value) scroller.value.scrollTop = scroller.value.scrollHeight;
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
async function remove() {
  const ok = await confirmBox('Delete this conversation?', 'Every message both ways and the files the player sent are deleted for good. They can write again.', 'Delete', true);
  if (!ok) return;
  busy.value = true;
  try {
    await api('DELETE', `/support/${openId.value}`);
    toast('Deleted.');
    reloadList();
    router.push('/support');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
async function setStatus(status) {
  busy.value = true;
  try {
    await api('PUT', `/support/${openId.value}/status`, { status });
    thread.value = { ...thread.value, status };
    reloadList();
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped>
.split { grid-template-columns: minmax(280px, 0.8fr) minmax(0, 1.4fr); align-items: start; }
.threads { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.thread {
  width: 100%; text-align: left; display: flex; flex-direction: column; align-items: stretch; gap: 4px; padding: 10px 12px;
  border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); color: var(--text); cursor: pointer; font: inherit;
}
.thread:hover { border-color: var(--line-2); }
.thread.picked { border-color: var(--accent); background: var(--accent-soft); }
.thread.unread b { color: var(--info); }
.between { display: flex; align-items: center; justify-content: space-between; gap: 8px; width: 100%; }
.tight { gap: 4px; flex: none; }
.last { color: var(--soft); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.nowrap { white-space: nowrap; }
.mono { font-family: var(--mono); }
.conversation header { align-items: flex-start; gap: 12px; flex-wrap: wrap; }
.thread-actions { display: flex; align-items: center; gap: 8px; }
.messages { display: flex; flex-direction: column; gap: 10px; max-height: 60vh; overflow-y: auto; padding: 4px 2px 12px; }
.msg { max-width: 82%; padding: 10px 12px; border-radius: 12px; border: 1px solid var(--line); background: var(--panel-2); }
.msg.admin { align-self: flex-end; border-color: var(--accent); background: var(--accent-soft); }
.meta { display: flex; gap: 8px; margin-bottom: 4px; }
.text { white-space: pre-wrap; word-break: break-word; }
.files { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
.answer { display: flex; flex-direction: column; gap: 8px; border-top: 1px solid var(--line); padding-top: 12px; }
.answer textarea { font-family: var(--sans); font-size: 14px; }
@media (max-width: 1100px) { .split { grid-template-columns: 1fr; } }
</style>
