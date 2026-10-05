<template>
  <PageTop title="Security" :tabs="SECURITY_TABS" sub="Audit log: sign-ins, failures and every change, newest first. Kept for 400 days.">
    <template #stats>
      <Stat :value="fmt.n(events.length)" label="Entries shown" :sub="more ? 'older ones below' : 'all of them'" />
      <Stat :value="fmt.n(today)" label="Last 24 hours" />
      <Stat :value="fmt.n(failures)" label="Failed sign-ins" sub="last 24 hours" :tone="failures ? 'warn' : ''" />
    </template>
  </PageTop>
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
import Stat from '../components/Stat.vue';
import { SECURITY_TABS } from '../router.js';
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
const today = computed(() => events.value.filter(e => e.at >= Date.now() / 1000 - 86400).length);
const failures = computed(() => events.value.filter(e => e.at >= Date.now() / 1000 - 86400 && /fail|refused/i.test(e.event)).length);
</script>
