<template>
  <PageTop title="Alerts" sub="Checked every minute. Raised when something goes wrong, resolved on their own when it's fixed." />
  <template v-if="data">
    <section class="panel">
      <header><h2>Open</h2><span class="small muted">{{ data.active.length ? `${data.active.length} open` : 'none' }}</span></header>
      <div v-if="data.active.length" class="stack">
        <div v-for="a in data.active" :key="a.id" class="callout" :class="a.level === 'bad' ? 'bad' : 'warn'">
          <div class="row" style="justify-content: space-between">
            <b>{{ a.detail }}</b>
            <span class="small muted">since {{ fmt.when(a.started_at) }} · {{ fmt.dur(now - a.started_at) }}</span>
          </div>
          <span class="small muted">{{ KINDS[a.kind] || a.kind }}{{ a.server ? ` · ${names.get(a.server) || a.server}` : '' }}</span>
        </div>
      </div>
      <div v-else class="empty">All clear.</div>
    </section>
    <section class="panel">
      <header><h2>Resolved</h2><span class="small muted">the last hundred</span></header>
      <div v-if="data.recent.length" class="table-wrap">
        <table>
          <thead><tr><th>Alert</th><th>What</th><th>Server</th><th>From</th><th>To</th><th class="r">For</th></tr></thead>
          <tbody>
            <tr v-for="a in data.recent" :key="a.id">
              <td><span class="dot" :class="a.level === 'bad' ? 'bad' : 'warn'"></span> {{ a.detail }}</td>
              <td class="muted">{{ KINDS[a.kind] || a.kind }}</td>
              <td class="muted">{{ a.server ? names.get(a.server) || a.server : '–' }}</td>
              <td class="small muted">{{ fmt.when(a.started_at) }}</td>
              <td class="small muted">{{ fmt.when(a.resolved_at) }}</td>
              <td class="r">{{ fmt.dur(a.resolved_at - a.started_at) || '<1m' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">Nothing yet.</div>
    </section>
    <section class="panel">
      <header><h2>Send alerts to a chat</h2><span class="small muted">{{ data.webhook_host ? `Sending to ${data.webhook_host}` : 'Off' }}</span></header>
      <form class="stack" @submit.prevent="save">
        <p class="small muted">A Discord or Slack incoming webhook (https). New and resolved alerts are posted there. Changing it asks for your second factor.</p>
        <div class="row">
          <input v-model="webhook" type="url" placeholder="https://discord.com/api/webhooks/…" autocomplete="off" style="flex: 1 1 320px">
          <button class="primary" type="submit" :disabled="!webhook.trim() && !data.webhook_host">{{ webhook.trim() || !data.webhook_host ? 'Save' : 'Turn off' }}</button>
          <button v-if="data.webhook_host" type="button" @click="test">Send a test</button>
        </div>
      </form>
      <div class="stack reports">
        <div class="row" style="justify-content: space-between">
          <div>
            <h3>Player reports</h3>
            <p class="small muted">New reports from players' launchers, one message each (a few a minute; more go together in one).</p>
          </div>
          <div class="seg" role="group" aria-label="Player reports to post">
            <button v-for="[id, label] in REPORT_MODES" :key="id" type="button" :aria-pressed="String(data.report_alerts === id)" @click="setReports(id)">{{ label }}</button>
          </div>
        </div>
      </div>
    </section>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const KINDS = { offline: 'Offline', cpu: 'CPU', memory: 'Memory', disk: 'Disk', signins: 'Refused sign-ins', allowance: 'Traffic allowance', update: 'Update', rollout: 'Rollout' };
onMounted(() => ensureOverview().catch(() => {}));
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));
const { data, error, reload } = useLoad(() => api('GET', '/alerts'), () => null, { refresh: false });
watch(() => live.alertTick, reload);
const webhook = ref('');
const now = ref(Date.now() / 1000);
let timer;
onMounted(() => { timer = setInterval(() => { now.value = Date.now() / 1000; }, 30000); });
onUnmounted(() => clearInterval(timer));

async function save() {
  try {
    await api('PUT', '/alerts/webhook', { url: webhook.value.trim() });
    toast(webhook.value.trim() ? 'Saved. Send a test to check it.' : 'Alerts won\'t be sent anywhere.');
    webhook.value = '';
    reload();
  } catch (e) { toast(e.message, true); }
}
const REPORT_MODES = [['off', 'Off'], ['problems', 'Bad or with problems'], ['all', 'All']];
async function setReports(mode) {
  try {
    data.value.report_alerts = (await api('PUT', '/alerts/reports', { mode })).report_alerts;
    toast({ off: 'Player reports won\'t be posted.', problems: 'Reports rated bad or with problems ticked will be posted.', all: 'Every report will be posted.' }[mode]);
  } catch (e) { toast(e.message, true); }
}
async function test() {
  try { toast((await api('POST', '/alerts/test', {})).message); } catch (e) { toast(e.message, true); }
}
</script>

<style scoped>
.reports { margin-top: 16px; padding-top: 14px; border-top: 1px solid var(--line); }
</style>
