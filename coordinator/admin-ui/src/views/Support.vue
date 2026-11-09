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
    <SupportThread v-if="openId" :identity="openId" :fresh-name="freshName" @changed="reloadList" @gone="gone" />
    <section v-else class="panel"><div class="empty">Pick a conversation.</div></section>
  </div>
</template>

<script setup>
// Players' conversations with the admins (newest first); picking one opens it beside the list
// (#/support/<identity>). The coordinator keeps them; the player's launcher reads the answers.
import { computed, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import SupportThread from '../components/SupportThread.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const FILTERS = [['active', 'Active'], ['open', 'Their turn: open'], ['waiting', 'Waiting on them'], ['resolved', 'Resolved'], ['all', 'All']];
const STATUS_LABEL = { open: 'Open', waiting: 'Waiting', resolved: 'Resolved' };
const STATUS_TONE = { open: 'warn', waiting: 'info', resolved: 'ok' };

const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const filter = ref('active');
const openId = computed(() => (/^[A-Z2-7]{52}$/.test(route.params.identity || '') ? route.params.identity : null));
// Starting a conversation from a player's page: their name comes along (?name=).
const freshName = computed(() => (typeof route.query.name === 'string' ? route.query.name.slice(0, 40) : ''));

const { data: list, error: listError, reload: reloadList } = useLoad(() => api('GET', `/support?status=${filter.value}`), () => filter.value, { refresh: false });
watch(() => live.supportTick, () => reloadList());

const pick = id => router.push(`/support/${id}`);
function gone() {
  reloadList();
  router.push('/support');
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
@media (max-width: 1100px) { .split { grid-template-columns: 1fr; } }
</style>
