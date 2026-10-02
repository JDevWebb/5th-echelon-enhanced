<template>
  <PageTop title="Audit log" sub="Sign-ins, failures and every change, newest first. Kept for 400 days." />
  <section class="panel">
    <div v-if="loaded && !events.length" class="empty">Nothing yet.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Admin</th><th>Event</th><th>Detail</th><th>From</th></tr></thead>
        <tbody>
          <tr v-for="e in events" :key="e.id" :class="{ fresh: e.fresh }">
            <td class="small muted" style="white-space: nowrap">{{ fmt.when(e.at) }}</td>
            <td><span v-if="e.admin">{{ e.admin }}</span><span v-else class="faint">–</span></td>
            <td>{{ e.event }}</td>
            <td class="small muted">{{ e.detail }}</td>
            <td class="small muted">{{ e.ip }}{{ e.country ? ' · ' + e.country : '' }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <div class="row" style="margin-top: 10px"><button v-if="more" class="small" type="button" @click="older">Older</button></div>
  </section>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import { api } from '../lib/api.js';
import { fmt } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const stored = ref([]);
const loaded = ref(false);
const more = ref(false);
const openedAt = Date.now() / 1000;
onMounted(async () => {
  const data = await api('GET', '/audit');
  stored.value = data.events;
  more.value = data.events.length >= 200;
  loaded.value = true;
});
async function older() {
  const next = await api('GET', `/audit?before=${stored.value.at(-1)?.id}`);
  stored.value = [...stored.value, ...next.events];
  more.value = next.events.length >= 200;
}
// New entries arrive live, on top.
const events = computed(() => {
  const ids = new Set(stored.value.map(e => e.id));
  const fresh = live.audit.filter(e => !ids.has(e.id)).map(e => ({ ...e, fresh: e.at >= openedAt }));
  return [...fresh, ...stored.value];
});
</script>
