<template>
  <template v-if="o && !sv">
    <PageTop title="Server not found" />
    <p><RouterLink to="/servers">← All servers</RouterLink></p>
  </template>
  <template v-else-if="sv">
    <PageTop :title="serverName(sv)" :sub="[sv.listing?.region, sv.listing?.host, `id ${sv.id}`].filter(Boolean).join(' · ')">
      <template #stats>
        <Stat :value="serverStatus(sv)[1]" label="Status" :tone="serverStatus(sv)[0]" :sub="sv.online ? `seen ${fmt.ago(sv.last_seen)}` : `since ${fmt.when(sv.last_seen)}`" />
        <Stat :value="fmt.n(sv.metrics?.players?.online)" label="Players online" :sub="`${fmt.n(sv.metrics?.players?.in_match)} in a match`" />
        <Stat :value="fmt.ms(sv.ping_ms)" label="Ping" sub="from the coordinator" />
        <Stat :value="sv.listing?.version || '?'" label="Release" :sub="fmt.dur(sv.metrics?.uptime_secs) + ' up'" />
      </template>
      <RangePicker v-model="range" :options="RANGES" />
      <button class="small" type="button" @click="purge">Release unused names</button>
      <button class="danger small" type="button" @click="remove">Remove</button>
    </PageTop>
    <p><RouterLink class="small" to="/servers">← All servers</RouterLink></p>
    <section v-if="rb" class="panel reboots">
      <header><h2>System reboots</h2><span class="small muted">for the system's updates: when it's empty in its quiet hours, or in a booked window</span></header>
      <div class="stack">
        <p v-if="!rb.pending_since" class="small">No reboot pending.</p>
        <p v-else class="callout" :class="rb.overdue ? 'warn' : ''">
          A reboot has been pending since {{ fmt.when(rb.pending_since) }}{{ rb.draining ? ': draining now, rebooting in a moment' : '' }}.
          <template v-if="rb.overdue"> It's been over three days: book a maintenance window, or let it reboot when empty at any hour.</template>
        </p>
        <p class="small">
          {{ rb.idle_since ? `Empty since ${fmt.when(rb.idle_since)}.` : 'In use now.' }}
          Quiet hours: <b>{{ rb.quiet }}</b> <span class="muted">({{ rb.quiet_source }})</span>.
          <template v-if="rb.any_hour_until"> Reboots when empty at any hour until {{ fmt.when(rb.any_hour_until) }}.</template>
        </p>
        <div class="row">
          <label class="small" for="rb-hours">Quiet hours (UTC)</label>
          <input id="rb-hours" v-model="hours" type="text" placeholder="learned from play, e.g. 4-7" class="hours">
          <button class="small" type="button" :disabled="rbBusy" @click="saveHours">Save</button>
          <button class="ghost small" type="button" :disabled="rbBusy" @click="anyHour(!rb.any_hour_until)">{{ rb.any_hour_until ? 'Only in quiet hours' : 'Reboot when empty, any hour' }}</button>
        </div>
      </div>
    </section>
    <div v-if="data" class="grid cols-2">
      <section v-for="c in charts" :key="c.title" class="panel">
        <header><h2>{{ c.title }}</h2><div class="legend"><span v-for="s in c.series" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
        <LineChart :series="c.series" :from="data.from" :to="data.to" :format="c.format" :max="c.max" />
      </section>
    </div>
    <div v-else class="empty">Loading…</div>
  </template>
  <template v-else>
    <PageTop title="Server" />
    <div class="empty">Loading…</div>
  </template>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import LineChart from '../components/LineChart.vue';
import RangePicker from '../components/RangePicker.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, PALETTE, RANGES, serverName, serverStatus } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ id: String });
const router = useRouter();
const range = ref(86400);
onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const sv = computed(() => o.value?.servers.find(x => x.id === props.id));
const { data } = useLoad(() => api('GET', `/series?range=${range.value}`), () => range.value);

const charts = computed(() => {
  const pts = data.value?.points?.[props.id] || [];
  const one = (name, field, color) => ({ name, color, points: pts.map(p => ({ t: p.t, v: p[field] })) });
  const pings = (data.value?.pings?.[props.id] || []).map(p => ({ t: p.t, v: p.ms }));
  const memTotal = sv.value?.metrics?.system?.mem_total;
  return [
    { title: 'Players', series: [one('Online', 'players', PALETTE[0]), one('In a match', 'in_match', PALETTE[1]), one('Relayed here', 'relayed_players', PALETTE[2])] },
    { title: 'CPU', series: [one('Machine', 'cpu', PALETTE[0]), one('Server process', 'process_cpu', PALETTE[2])], format: v => fmt.pct(v), max: 100 },
    { title: 'Memory', series: [one('Used', 'mem_used', PALETTE[0]), one('Server process', 'rss', PALETTE[2])], format: fmt.bytes, max: memTotal || undefined },
    { title: 'Bandwidth', series: [one('In', 'rx', PALETTE[1]), one('Out', 'tx', PALETTE[0]), one('Relayed', 'relayed', PALETTE[2])], format: fmt.rate },
    { title: 'Sign-ins per minute', series: [one('Game', 'logins', PALETTE[0]), one('Failed', 'failed_logins', PALETTE[4]), one('New accounts', 'registrations', PALETTE[1])], format: v => fmt.n(v, 1) },
    { title: 'Ping from the coordinator', series: [{ name: 'Round trip', color: PALETTE[3], points: pings }], format: fmt.ms },
  ];
});

// Reboots for the system's updates (0.5.2): pending, empty since, quiet hours.
const rb = ref(null);
const hours = ref('');
const rbBusy = ref(false);
async function loadReboot() {
  try {
    rb.value = await api('GET', `/servers/${encodeURIComponent(props.id)}/reboot`);
    hours.value = rb.value.quiet_set || '';
  } catch { rb.value = null; }
}
onMounted(loadReboot);
async function rebootSettings(body, done) {
  rbBusy.value = true;
  try { await api('POST', `/servers/${encodeURIComponent(props.id)}/reboot`, body); toast(done); await loadReboot(); } catch (e) { toast(e.message, true); } finally { rbBusy.value = false; }
}
const saveHours = () => rebootSettings({ quiet_hours: hours.value.trim() }, hours.value.trim() ? `Quiet hours: ${hours.value.trim()} UTC.` : 'Quiet hours are learned from play.');
const anyHour = on => rebootSettings({ any_hour: on }, on ? 'It reboots when empty, at any hour, for three days.' : 'It reboots only in its quiet hours (or a window).');

async function purge() {
  if (!await confirmBox(`Release ${serverName(sv.value)}'s unused names?`, 'Its links made over an hour ago for identities never seen online, and linked nowhere else, go with the names only they held. For a server reserving names with throwaway identities.', 'Release names', true)) return;
  try { toast((await api('POST', `/servers/${encodeURIComponent(props.id)}/purge-names`)).message); } catch (e) { toast(e.message, true); }
}
async function remove() {
  if (!await confirmBox(`Remove ${serverName(sv.value)}?`, 'Its links and the names only it used go. It can join again with the join token unless you make a new one.', 'Remove server', true)) return;
  try { toast((await api('DELETE', `/servers/${encodeURIComponent(props.id)}`)).message); router.push('/servers'); } catch (e) { toast(e.message, true); }
}
</script>

<style scoped>
.reboots { margin-bottom: 16px; }
.hours { max-width: 180px; }
</style>
