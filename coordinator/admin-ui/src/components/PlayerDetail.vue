<template>
  <section class="panel detail">
    <header>
      <div>
        <h2 class="row" style="gap: 8px">
          <span class="dot" :class="{ ok: p?.online }" :title="p?.online ? 'online' : 'offline'"></span>{{ p?.name || '…' }}
          <span v-if="p?.banned" class="pill bad">banned</span>
        </h2>
        <p class="small muted">{{ names.get(server) || server }} · #{{ id }}{{ p?.identity ? ` · identity ${p.identity.slice(0, 12)}…` : ' · no identity' }}</p>
      </div>
      <button class="ghost small" type="button" aria-label="Close" @click="$emit('close')">✕</button>
    </header>
    <div v-if="!d" class="empty">{{ error || 'Loading…' }}</div>
    <template v-else>
      <p v-if="p.banned" class="callout bad">Banned {{ p.banned.until ? `until ${fmt.when(p.banned.until)}` : 'for good' }}{{ p.banned.reason ? `: ${p.banned.reason}` : '' }} <span class="muted small">(since {{ fmt.when(p.banned.at) }})</span></p>
      <div class="facts">
        <div class="fact"><b>{{ fmt.dur(p.play_seconds) }}</b><span>played in all</span></div>
        <div class="fact"><b>{{ fmt.dur(p.week_seconds) }}</b><span>last 7 days</span></div>
        <div class="fact"><b>{{ p.online ? 'now' : fmt.ago(p.last_seen) }}</b><span>last seen</span></div>
        <div class="fact"><b>{{ fmt.n(p.sessions) }}</b><span>sessions</span></div>
        <div class="fact"><b>{{ fmt.n(p.matches) }}</b><span>matches</span></div>
        <div class="fact"><b>{{ p.created_at ? new Date(p.created_at * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' }) : '–' }}</b><span>account made</span></div>
      </div>
      <div class="row actions">
        <button class="small" type="button" :disabled="!p.online" @click="act('kick')">Kick</button>
        <button v-if="!p.banned" class="small danger" type="button" @click="act('ban')">Ban</button>
        <button v-else class="small" type="button" @click="act('unban')">Unban</button>
        <button class="small" type="button" @click="act('reset_password')">Reset password</button>
        <button class="small" type="button" @click="act('rename')">Rename</button>
        <button class="small danger" type="button" @click="act('delete')">Delete</button>
      </div>
      <p v-for="(f, aid) in mine" :key="aid" class="callout" :class="{ bad: f.status === 'failed' || f.status === 'expired' }">
        {{ ACTION_LABELS[f.kind] }}: {{ f.status === 'pending' ? 'waiting for the server (it checks every 10 s)…' : f.message || f.status }}
      </p>

      <h3 class="sub">Time played, last 30 days</h3>
      <LineChart :series="daySeries" :from="d.days[0].t" :to="d.days.at(-1).t" :format="v => `${fmt.n(v)} min`" :height="140" />

      <template v-if="d.others.length">
        <h3 class="sub">Other accounts (same identity)</h3>
        <ul class="list">
          <li v-for="o in d.others" :key="o.server + o.id">
            <button class="ghost small" type="button" @click="$emit('open', o)">{{ o.name }}</button>
            <span class="small muted">{{ names.get(o.server) || o.server }} · {{ o.online ? 'online' : fmt.ago(o.last_seen) }}{{ o.banned ? ' · banned' : '' }}</span>
          </li>
        </ul>
      </template>

      <h3 class="sub">Sessions</h3>
      <div v-if="d.sessions.length" class="table-wrap">
        <table>
          <thead><tr><th>Started</th><th>Ended</th><th class="r">Length</th></tr></thead>
          <tbody>
            <tr v-for="s in d.sessions" :key="s.id">
              <td>{{ fmt.when(s.start) }}</td>
              <td class="muted">{{ s.end ? fmt.when(s.end) : 'playing' }}</td>
              <td class="r">{{ fmt.dur(s.seconds) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No sessions reported (older servers don't send them).</div>

      <h3 class="sub">Admin actions</h3>
      <div v-if="d.actions.length" class="table-wrap">
        <table>
          <thead><tr><th>Action</th><th>By</th><th>When</th><th>Result</th></tr></thead>
          <tbody>
            <tr v-for="a in d.actions" :key="a.id">
              <td>{{ ACTION_LABELS[a.kind] || a.kind }}<span v-if="a.args?.name" class="muted"> to {{ a.args.name }}</span><span v-if="a.args?.reason" class="muted small"> · {{ a.args.reason }}</span></td>
              <td class="muted">{{ a.created_by }}</td>
              <td class="muted">{{ fmt.ago(a.created_at) }}</td>
              <td><span class="pill" :class="{ ok: a.status === 'done', bad: a.status === 'failed' || a.status === 'expired' }">{{ a.status }}</span> <span class="small muted">{{ a.message }}</span></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">None yet.</div>
    </template>
  </section>
</template>

<script setup>
// One player: what they played, their other accounts, and the actions admins can take.
import { computed, ref, watch } from 'vue';
import LineChart from './LineChart.vue';
import PlayerAction from './PlayerAction.vue';
import TempPassword from './TempPassword.vue';
import { api } from '../lib/api.js';
import { ACTION_LABELS, follow, following } from '../lib/actions.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, PALETTE } from '../lib/fmt.js';
import { openModal, toast } from '../lib/ui.js';

const props = defineProps({ server: String, id: Number, names: Map });
const emit = defineEmits(['close', 'changed', 'open']);
const d = ref(null);
const error = ref('');
const p = computed(() => d.value?.player);
// Actions taken here, while they're followed.
const mineIds = ref([]);
const mine = computed(() => Object.fromEntries(mineIds.value.filter(aid => following[aid]).map(aid => [aid, following[aid]])));

async function load() {
  try {
    d.value = await api('GET', `/players/${encodeURIComponent(props.server)}/${props.id}`);
    error.value = '';
  } catch (e) {
    error.value = e.status === 404 ? 'This player is gone (deleted on their server).' : e.message;
    d.value = null;
  }
}
watch(() => [props.server, props.id], () => { d.value = null; mineIds.value = []; load(); }, { immediate: true });

const daySeries = computed(() => [{ name: 'Played', color: PALETTE[0], points: d.value.days.map(x => ({ t: x.t, v: x.seconds / 60 })) }]);

async function act(kind) {
  let queued;
  if (kind === 'kick' || kind === 'unban') {
    const ok = await confirmBox(`${ACTION_LABELS[kind]} ${p.value.name}?`, kind === 'kick' ? 'They\'re signed out, and can sign in again.' : 'They can sign in again.', ACTION_LABELS[kind]);
    if (!ok) return;
    try {
      queued = (await api('POST', `/players/${encodeURIComponent(props.server)}/${props.id}/actions`, { kind })).actions;
    } catch (e) { toast(e.message, true); return; }
  } else {
    queued = await openModal(PlayerAction, { kind, player: p.value, server: props.names.get(props.server) || props.server });
    if (!queued) return;
  }
  const who = p.value.name;
  load();
  for (const a of queued) {
    mineIds.value = [...mineIds.value, a.id];
    follow(a.id, kind).then(done => {
      toast(`${ACTION_LABELS[kind]} ${who}: ${done.message || done.status}`, done.status !== 'done');
      if (done.password) openModal(TempPassword, { who, password: done.password });
      load();
      emit('changed');
    }).catch(e => toast(e.message, true));
  }
}
</script>

<style scoped>
.detail { position: sticky; top: 16px; max-height: calc(100vh - 32px); overflow-y: auto; }
.sub { margin: 18px 0 8px; font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }
.actions { margin-top: 14px; }
.callout { margin-top: 10px; }
.list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.list li { display: flex; align-items: center; gap: 8px; }
@media (max-width: 1100px) { .detail { position: static; max-height: none; } }
</style>
