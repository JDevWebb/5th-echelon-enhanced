<template>
  <PageTop title="Moderation" sub="Chat reports from players, mutes and the rules for chat and names. Slurs and hate are masked in chat and refused in names; links are refused but a Discord invite.">
    <template #stats>
      <Stat v-if="data?.chat" :value="fmt.n(data.reports.length)" label="Open reports" :tone="data.reports.length ? 'info' : ''" />
      <Stat v-if="data?.chat" :value="fmt.n(data.mutes.length)" label="Mutes" />
      <Stat :value="data ? fmt.n(data.names.length) : '–'" label="Names failing" :tone="data?.names.length ? 'warn' : ''" />
    </template>
    <div class="seg" role="group" aria-label="Show">
      <button v-for="[id, label] in tabs" :key="id" type="button" :aria-pressed="String(tab === id)" @click="tab = id">{{ label }}</button>
    </div>
  </PageTop>

  <div v-if="!data" class="panel empty">{{ error || 'Loading…' }}</div>

  <!-- Reports: one a message, oldest first. -->
  <template v-else-if="tab === 'reports'">
    <div v-if="!data.reports.length" class="panel empty">No open reports. Players report a message from the launcher's or the overlay's chat.</div>
    <section v-for="r in data.reports" :key="r.message.id" class="panel report">
      <header>
        <div>
          <h2>{{ ROOM[r.message.room] }} <span class="small">· {{ fmt.ago(r.reports[0].at) }}</span></h2>
          <p class="small muted">
            Reported by <template v-for="(p, i) in r.reports" :key="p.identity"><template v-if="i">, </template><b>{{ p.name || short(p.identity) }}</b><template v-if="p.why">: “{{ p.why }}”</template></template>
          </p>
        </div>
        <span v-if="r.message.deleted_at" class="pill">Deleted by {{ r.message.deleted_by === 'sender' ? 'the sender' : r.message.deleted_by.slice(6) }}</span>
      </header>
      <p v-if="r.message.room === 'direct'" class="small muted">A direct conversation: only the messages around this one are shown.</p>
      <ol class="chat">
        <li v-for="m in r.before" :key="m.id" class="context"><ChatLine :m="m" /></li>
        <li class="reported"><ChatLine :m="r.message" /></li>
        <li v-for="m in r.after" :key="m.id" class="context"><ChatLine :m="m" /></li>
      </ol>
      <div class="sender small">
        <span class="muted">{{ r.message.from_name }}</span>
        <template v-for="a in r.sender.accounts" :key="a.server + a.id"> · <RouterLink :to="`/players/${a.server}/${a.id}`">{{ a.name }} on {{ names.get(a.server) || a.server }}</RouterLink></template>
        <span v-if="!r.sender.accounts.length" class="muted"> · no accounts listed</span>
        <span v-for="mu in r.sender.mutes" :key="mu.room" class="pill warn">Muted {{ muteWhere(mu.room) }} {{ mu.until ? 'until ' + fmt.when(mu.until) : 'for good' }}</span>
      </div>
      <div class="act row">
        <input v-model="notes[r.message.id]" class="note" type="text" maxlength="300" placeholder="Note (optional)" aria-label="Note" />
        <button class="small danger" type="button" :disabled="busy || !!r.message.deleted_at" @click="deal(r, 'delete')">Delete message</button>
        <button class="small" type="button" :disabled="busy" @click="deal(r, 'resolve')">Dealt with</button>
        <button class="small" type="button" :disabled="busy" @click="deal(r, 'dismiss')">Dismiss</button>
        <span class="spacer" />
        <MuteForm :disabled="busy" @mute="m => mute(r.message.from, r.message.from_name, m)" />
      </div>
    </section>
  </template>

  <!-- Messages refused for a link, kept 7 days. -->
  <section v-else-if="tab === 'refused'" class="panel">
    <header><h2>Refused for a link · kept 7 days</h2></header>
    <div v-if="!data.refused.length" class="empty">None this week.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Player</th><th>Server</th><th>Room</th><th>Found</th><th>Message</th></tr></thead>
        <tbody>
          <tr v-for="x in data.refused" :key="x.id">
            <td class="nowrap" :title="fmt.when(x.at)">{{ fmt.ago(x.at) }}</td>
            <td>{{ x.from_name }}</td>
            <td>{{ names.get(x.server) || x.server }}</td>
            <td>{{ ROOM[x.room] || x.room }}</td>
            <td class="mono">{{ x.reason }}</td>
            <td class="body">{{ x.body }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>

  <!-- Mutes in force, and who has the most masked words this week. -->
  <div v-else-if="tab === 'mutes'" class="grid two">
    <section class="panel">
      <header><h2>Mutes in force</h2></header>
      <div v-if="!data.mutes.length" class="empty">Nobody is muted. Mute from a report, or from the list beside.</div>
      <div v-else class="table-wrap">
        <table>
          <thead><tr><th>Player</th><th>Where</th><th>Until</th><th>By</th><th></th></tr></thead>
          <tbody>
            <tr v-for="mu in data.mutes" :key="mu.identity + mu.room">
              <td>{{ mu.name || short(mu.identity) }}<div v-if="mu.reason" class="small muted">{{ mu.reason }}</div></td>
              <td>{{ muteWhere(mu.room) }}</td>
              <td class="nowrap">{{ mu.until ? fmt.when(mu.until) : 'For good' }}</td>
              <td>{{ mu.by }}</td>
              <td><button class="small" type="button" :disabled="busy" @click="unmute(mu)">Lift</button></td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <section class="panel">
      <header><h2>Most masked this week</h2></header>
      <div v-if="!data.masked.length" class="empty">No words masked this week.</div>
      <ul v-else class="masked">
        <li v-for="x in data.masked" :key="x.identity" class="row">
          <b>{{ x.name || short(x.identity) }}</b>
          <span class="pill">{{ fmt.n(x.count) }} masked</span>
          <span class="spacer" />
          <MuteForm :disabled="busy" @mute="m => mute(x.identity, x.name, m)" />
        </li>
      </ul>
    </section>
  </div>

  <!-- Names that fail the rules: nobody is renamed by them. -->
  <section v-else-if="tab === 'names'" class="panel">
    <header><h2>Names failing the rules</h2></header>
    <p class="small muted">Names players had before the rules, or that a change of the rules catches. New names are refused already. Rename a player from their page, or allow a name the rules take for something else.</p>
    <div v-if="!data.names.length" class="empty">Every player's name passes.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>Name</th><th>Server</th><th>Why</th><th>Last seen</th><th></th></tr></thead>
        <tbody>
          <tr v-for="n in data.names" :key="n.server + n.id">
            <td><RouterLink :to="`/players/${n.server}/${n.id}`">{{ n.name }}</RouterLink></td>
            <td>{{ names.get(n.server) || n.server }}</td>
            <td class="small">{{ n.why }}</td>
            <td class="nowrap">{{ n.last_seen ? fmt.ago(n.last_seen) : '–' }}</td>
            <td><button class="small" type="button" :disabled="busy" @click="allowName(n.name)">Allow</button></td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>

  <!-- The rules' settings. -->
  <section v-else-if="tab === 'rules'" class="panel">
    <header><h2>Rules</h2></header>
    <form class="rules" @submit.prevent="saveRules">
      <div class="grid two">
        <label class="field">
          <span>Allowed words and names, one a line</span>
          <textarea v-model="allowText" rows="10" spellcheck="false" placeholder="A word or name the rules wrongly catch"></textarea>
        </label>
        <label class="field">
          <span>The network's own words, one a line</span>
          <textarea v-model="extraText" rows="10" spellcheck="false" placeholder="Masked in chat, refused in names (looked for inside names too)"></textarea>
        </label>
      </div>
      <label v-if="data.chat" class="check"><input v-model="invites" type="checkbox" /> Discord invites may be sent (one a message; one every 10 minutes in Global)</label>
      <div class="row">
        <button class="primary" type="submit" :disabled="busy">Save</button>
        <span class="small muted">Servers use them from their next pulse (within a minute). Saving asks you to confirm it's you.</span>
      </div>
    </form>
  </section>
</template>

<script setup>
// Chat reports (one a message, with what came before it), messages refused for a link, mutes
// and the most masked, names failing the rules, and the rules' settings. The chat parts are on
// the community network's coordinator only (with --chat); names and rules are everywhere.
import { computed, defineComponent, h, onMounted, reactive, ref, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const ROOM = { global: 'Global', direct: 'Direct message', lobby: 'Lobby' };
const short = id => (id ? `${id.slice(0, 4)}-${id.slice(-4)}` : '');
const muteWhere = room => (room === '*' ? 'everywhere' : `in ${ROOM[room] || room}`);

/** One chat line: who, when, what (masked words show as they were sent: ***). */
const ChatLine = defineComponent({
  props: { m: { type: Object, required: true } },
  setup: p => () => h('div', { class: 'line' }, [
    h('span', { class: 'from' }, p.m.from_name),
    h('span', { class: 'when muted small', title: fmt.when(p.m.at) }, fmt.ago(p.m.at)),
    h('span', { class: ['what', { gone: p.m.deleted_at }] }, p.m.body),
  ]),
});

/** Mute: where and for how long. */
const MuteForm = defineComponent({
  props: { disabled: Boolean },
  emits: ['mute'],
  setup(p, { emit }) {
    const room = ref('*');
    const hours = ref('24');
    return () => h('span', { class: 'row tight' }, [
      h('select', { value: room.value, 'aria-label': 'Where', onChange: e => { room.value = e.target.value; } },
        [['*', 'Everywhere'], ['global', 'Global'], ['direct', 'Direct'], ['lobby', 'Lobby']].map(([v, l]) => h('option', { value: v }, l))),
      h('select', { value: hours.value, 'aria-label': 'For', onChange: e => { hours.value = e.target.value; } },
        [['1', '1 hour'], ['24', '1 day'], ['168', '7 days'], ['', 'For good']].map(([v, l]) => h('option', { value: v }, l))),
      h('button', { class: 'small', type: 'button', disabled: p.disabled, onClick: () => emit('mute', { room: room.value, hours: hours.value ? Number(hours.value) : null }) }, 'Mute'),
    ]);
  },
});

onMounted(() => ensureOverview().catch(() => {}));
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));
const { data, error, reload } = useLoad(() => api('GET', '/moderation'), () => null, { refresh: false });
watch(() => live.moderationTick, reload);

const tabs = computed(() => (data.value?.chat
  ? [['reports', 'Reports'], ['refused', 'Refused'], ['mutes', 'Mutes'], ['names', 'Names'], ['rules', 'Rules']]
  : [['names', 'Names'], ['rules', 'Rules']]));
const tab = ref('reports');
watch(tabs, t => { if (!t.some(([id]) => id === tab.value)) tab.value = t[0][0]; });

const busy = ref(false);
const notes = reactive({});
async function run(work, done) {
  busy.value = true;
  try {
    await work();
    if (done) toast(done);
    await reload();
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}

async function deal(r, action) {
  if (action === 'delete') {
    const ok = await confirmBox('Delete this message?', `${r.message.from_name}'s message goes for everyone who can see it. The report is kept 90 days.`, 'Delete', true);
    if (!ok) return;
  }
  await run(() => api('POST', `/moderation/reports/${r.message.id}`, { action, note: notes[r.message.id] || '' }),
    { delete: 'Deleted.', resolve: 'Marked dealt with.', dismiss: 'Dismissed.' }[action]);
}

async function mute(identity, name, { room, hours }) {
  const ok = await confirmBox(`Mute ${name || 'this player'}?`,
    `${muteWhere(room)[0].toUpperCase()}${muteWhere(room).slice(1)}, ${hours ? `for ${hours === 1 ? 'an hour' : hours === 24 ? 'a day' : `${hours / 24} days`}` : 'for good'}. They're told when they try to post.`, 'Mute', true);
  if (!ok) return;
  await run(() => api('POST', '/moderation/mutes', { identity, room, hours }), 'Muted.');
}
const unmute = mu => run(() => api('DELETE', `/moderation/mutes/${mu.identity}/${encodeURIComponent(mu.room)}`), 'Lifted.');

// The rules' settings, editable copies of what's saved.
const allowText = ref('');
const extraText = ref('');
const invites = ref(true);
watch(data, d => {
  if (!d) return;
  allowText.value = d.settings.allow.join('\n');
  extraText.value = d.settings.extra.join('\n');
  invites.value = d.settings.discord_invites;
}, { immediate: true });
const lines = t => t.split('\n').map(s => s.trim()).filter(Boolean);
const saveRules = () => run(() => api('PUT', '/moderation/settings', { allow: lines(allowText.value), extra: lines(extraText.value), discord_invites: invites.value }), 'Saved.');
const allowName = name => run(() => api('PUT', '/moderation/settings', {
  allow: [...data.value.settings.allow, name.toLowerCase()],
  extra: data.value.settings.extra,
  discord_invites: data.value.settings.discord_invites,
}), `${name} allowed.`);
</script>

<style scoped>
.two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.report header { align-items: flex-start; }
.chat { list-style: none; margin: 0 0 10px; padding: 8px 10px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); display: flex; flex-direction: column; gap: 4px; }
.chat :deep(.line) { display: flex; gap: 8px; align-items: baseline; }
.chat :deep(.from) { font-weight: 600; flex: none; }
.chat :deep(.what) { white-space: pre-wrap; word-break: break-word; }
.chat :deep(.gone) { text-decoration: line-through; color: var(--muted); }
.context { color: var(--soft); }
.reported { border-left: 3px solid var(--bad); background: var(--bad-soft); margin: 2px -10px; padding: 4px 7px; }
.sender { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin-bottom: 10px; }
.act { gap: 8px; }
.note { flex: 1 1 200px; min-width: 160px; }
.spacer { flex: 1; }
.tight { gap: 4px; }
.report :deep(select), .masked :deep(select) { width: auto; }
.report :deep(.row.tight), .masked :deep(.row.tight) { flex-wrap: nowrap; }
.masked { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
.body { word-break: break-word; }
.nowrap { white-space: nowrap; }
.mono { font-family: var(--mono); }
.rules { display: flex; flex-direction: column; gap: 14px; }
.rules textarea { font-family: var(--mono); font-size: 13px; }
.check { display: flex; gap: 8px; align-items: center; }
@media (max-width: 1100px) { .two { grid-template-columns: 1fr; } }
</style>
