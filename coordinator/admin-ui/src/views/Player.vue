<template>
  <div class="back"><RouterLink to="/players">← Players</RouterLink></div>
  <div v-if="!d" class="panel"><div class="empty">{{ error || 'Loading…' }}</div></div>
  <template v-else>
    <header class="profile">
      <div class="who">
        <span class="avatar big" :class="{ on: p.online }" aria-hidden="true">{{ p.name.slice(0, 2) }}</span>
        <div class="who-text">
          <div class="row" style="gap: 10px">
            <h1>{{ p.name }}</h1>
            <span class="pill" :class="p.online ? 'ok' : ''"><i class="dot" :class="{ ok: p.online }"></i> {{ p.online ? 'Online' : 'Offline' }} · {{ serverLabel }}</span>
          </div>
          <div class="row pills">
            <span v-if="d.network_ban" class="pill bad">Banned on every server</span>
            <span v-else-if="p.banned" class="pill bad">Banned here</span>
            <span v-for="mu in d.mutes" :key="mu.room" class="pill warn">Muted {{ muteWhere(mu.room) }}</span>
            <span v-if="support" class="pill" :class="support.status === 'open' ? 'warn' : 'info'">Support {{ support.status === 'waiting' ? 'waiting on them' : support.status }}</span>
            <span v-if="flags.length" class="pill warn">Fair play: {{ flags.length }} flag{{ flags.length === 1 ? '' : 's' }}</span>
            <span v-if="d.others.length" class="pill">{{ d.others.length + 1 }} accounts</span>
            <span v-if="!d.network_ban && !p.banned && !d.mutes.length && !flags.length" class="pill">No bans, mutes or flags</span>
          </div>
          <div class="small faint">
            #{{ p.id }}<template v-if="p.created_at"> · account made {{ day(p.created_at) }}</template>
            · last seen {{ p.online ? 'now' : fmt.ago(p.last_seen) }}
            <template v-if="p.identity"> · <span class="mono" :title="p.identity">identity {{ p.identity.slice(0, 4) }}-{{ p.identity.slice(-4) }}</span></template>
            <template v-else> · no identity</template>
          </div>
        </div>
      </div>
      <div class="actbar" role="group" aria-label="Actions">
        <RouterLink v-if="features.roadmap && p.identity" class="btn primary" :to="{ path: `/support/${p.identity}`, query: { name: p.name } }" title="They read it on their launcher's Support page, and the game's overlay tells them">{{ support ? 'Open support' : 'Message' }}</RouterLink>
        <button type="button" :disabled="!p.online" @click="act('kick')">Kick</button>
        <button v-if="!p.banned && !d.network_ban" class="danger" type="button" @click="act('ban')">Ban…</button>
        <button v-else type="button" @click="act('unban')">Unban</button>
        <details ref="more" class="more">
          <summary class="btn" aria-label="More actions">More</summary>
          <div class="menu" @click="closeMore">
            <button type="button" @click="act('rename')">Rename…</button>
            <button type="button" title="Their launcher and overlay ask them to choose another name, and the game is kept out until they do" @click="act('name_not_allowed')">Ask to rename</button>
            <button type="button" title="Their name was flagged by mistake: no longer asked to rename, and not flagged again" @click="act('allow_name')">Allow name</button>
            <button type="button" @click="act('reset_password')">Reset password…</button>
            <button v-if="p.identity" type="button" title="A developer: every server takes their identity's other launcher and add-on builds (Security › Client builds)" @click="allowBuilds">Allow other builds</button>
            <button class="danger" type="button" @click="act('delete')">Delete account…</button>
          </div>
        </details>
      </div>
    </header>

    <p v-for="(f, aid) in mine" :key="aid" class="callout" :class="{ bad: f.status === 'failed' || f.status === 'expired' }">
      {{ ACTION_LABELS[f.kind] }}: {{ f.status === 'pending' ? 'waiting for the server (it checks every 10 s)…' : f.message || f.status }}
    </p>

    <nav class="ptabs" aria-label="Player">
      <button v-for="[id, label, n] in tabs" :key="id" type="button" class="tab" :aria-current="tab === id ? 'page' : null" @click="setTab(id)">
        {{ label }}<b v-if="n" class="count">{{ n }}</b>
      </button>
    </nav>

    <div class="layout">
      <div class="col">
        <!-- Activity: the last 5 days, as the Sessions page sees them, and what went wrong. -->
        <template v-if="tab === 'activity'">
          <section class="panel">
            <header>
              <h2>The last 5 days</h2>
              <div class="legend">
                <span><i class="sw online"></i>Online</span>
                <span><i class="sw svm"></i>Spies vs Mercs</span>
                <span><i class="sw coop"></i>Co-op</span>
                <span><i class="sw party"></i>Party</span>
                <span><i class="tick bad"></i>Problem</span>
                <span><i class="tick warn"></i>Warning</span>
              </div>
            </header>
            <div v-if="!act5" class="empty">{{ act5Error || 'Loading…' }}</div>
            <template v-else>
              <div class="hours"><span v-for="h in [0, 4, 8, 12, 16, 20]" :key="h">{{ String(h).padStart(2, '0') }}:00</span></div>
              <div v-for="day in days" :key="day.start" class="day">
                <span class="small">{{ day.label }}</span>
                <div class="track">
                  <span v-for="(o, i) in day.online" :key="'n' + i" class="seg online" :style="o.style" :title="o.tip"></span>
                  <span v-for="(r, i) in day.rooms" :key="'r' + i" class="seg room" :class="r.cls" :style="r.style" :title="r.tip"></span>
                  <span v-for="(m, i) in day.marks" :key="'m' + i" class="mark" :class="m.cls" :style="m.style" :title="m.tip"></span>
                </div>
              </div>
            </template>
          </section>
          <section class="panel">
            <header><h2>What went wrong</h2><RouterLink class="small" to="/sessions">Sessions page</RouterLink></header>
            <div v-if="!act5" class="empty">Loading…</div>
            <div v-else-if="!problems.length" class="empty">Nothing in the last 5 days.</div>
            <ul v-else class="rows">
              <li v-for="(x, i) in problems" :key="i">
                <span class="pill" :class="x.level">{{ x.level === 'bad' ? 'Problem' : x.level === 'warn' ? 'Warning' : 'Note' }}</span>
                <span class="grow"><b>{{ x.title }}</b><br><span class="small muted">{{ x.text }}</span></span>
                <span class="small faint mono nowrap">{{ when(x.at) }}</span>
              </li>
            </ul>
          </section>
        </template>

        <template v-else-if="tab === 'sessions'">
          <section class="panel">
            <header><h2>Time played, last 30 days</h2></header>
            <LineChart :series="daySeries" :from="d.days[0].t" :to="d.days.at(-1).t" :format="v => `${fmt.n(v, v > 0 && v < 10 ? 1 : 0)} min`" :height="140" />
          </section>
          <section class="panel">
            <header><h2>Sessions</h2><span class="small muted">the last 50</span></header>
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
          </section>
        </template>

        <template v-else-if="tab === 'moderation'">
          <p v-if="d.network_ban" class="callout bad">Banned across the network {{ d.network_ban.until ? `until ${fmt.when(d.network_ban.until)}` : 'for good' }}{{ d.network_ban.reason ? `: ${d.network_ban.reason}` : '' }}: every account of theirs, on every server, and any they make. <span class="muted small">(by {{ d.network_ban.by }}, {{ fmt.when(d.network_ban.at) }})</span></p>
          <p v-else-if="p.banned" class="callout bad">Banned {{ p.banned.until ? `until ${fmt.when(p.banned.until)}` : 'for good' }}{{ p.banned.reason ? `: ${p.banned.reason}` : '' }} <span class="muted small">(since {{ fmt.when(p.banned.at) }})</span></p>
          <section class="panel">
            <header><h2>Chat mutes</h2></header>
            <ul v-if="d.mutes.length" class="rows">
              <li v-for="mu in d.mutes" :key="mu.room">
                <span class="pill warn">Muted {{ muteWhere(mu.room) }}</span>
                <span class="grow small muted">{{ mu.until ? 'until ' + fmt.when(mu.until) : 'for good' }} · {{ mu.by }} · {{ fmt.ago(mu.at) }}{{ mu.reason ? ` · ${mu.reason}` : '' }}</span>
                <button class="ghost small" type="button" title="They can post there again at once" @click="unmute(mu)">Lift</button>
              </li>
            </ul>
            <div v-else class="empty">None in force. Mute from a reported message on Chat and names.</div>
          </section>
          <section class="panel">
            <header><h2>Admin actions</h2></header>
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
          </section>
        </template>

        <template v-else-if="tab === 'fair'">
          <section class="panel">
            <header><h2>Their PC</h2><span class="small muted">what it shares with banned players', never the ids themselves</span></header>
            <div v-if="!flags.length" class="empty">Nothing found.</div>
            <div v-else class="stack">
              <p v-for="m in evasions" :key="`evasion${m.server}${m.player}`" class="callout warn">
                Maybe banned <RouterLink :to="`/players/${encodeURIComponent(m.server)}/${m.player}`">{{ m.name || `#${m.player}` }}</RouterLink> back under a new name: {{ m.detail }}. Only flagged: networks and friends are shared.
                <span class="muted small">({{ fmt.when(m.at) }})</span>
              </p>
              <p v-for="m in hwMatches" :key="`${m.outcome}${m.server}${m.player}`" class="callout" :class="m.outcome === 'ban' ? 'bad' : 'warn'">
                {{ m.outcome === 'ban' ? 'Banned for their PC' : 'Maybe the same PC' }}: {{ m.matched }} hardware id{{ m.matched === 1 ? '' : 's' }} in common with banned
                <RouterLink :to="`/players/${encodeURIComponent(m.server)}/${m.player}`">{{ m.name || `#${m.player}` }}</RouterLink> on {{ m.server }}{{ m.outcome === 'ban' ? '' : ' (one id: a shared or second-hand PC is possible, so it\'s only flagged)' }}.
                <span class="muted small">({{ fmt.when(m.at) }})</span>
              </p>
              <p v-if="d.hardware?.vm" class="callout warn">Plays on a virtual machine ({{ d.hardware.vm }}){{ (d.hardware.matches || []).some(m => m.outcome === 'vm') ? ': flagged as a new account on one, a way round a ban on a PC' : '' }}.</p>
            </div>
          </section>
        </template>

        <template v-else-if="tab === 'loadouts'">
          <section class="panel">
            <header><h2>Loadouts and progress</h2><span class="small muted">admins only</span></header>
            <div v-if="snap">
              <p class="small muted">The game's ShadowNet snapshot, as of {{ fmt.when(snap.updated_at) }} ({{ Math.round(snap.size / 1024) }} KB). Items and challenges are the game's ids for now.</p>
              <div class="row" style="gap: 6px; margin: 6px 0">
                <span class="pill">{{ fmt.n(snap.summary.items) }} items owned</span>
                <span class="pill">{{ fmt.n(snap.summary.loadouts) }} loadouts</span>
                <span class="pill">{{ fmt.n(snap.summary.purchases) }} purchases</span>
                <span class="pill">{{ fmt.n(snap.summary.challenges_started) }} of {{ fmt.n(snap.summary.challenges) }} challenges started</span>
              </div>
              <details>
                <summary class="small">The snapshot</summary>
                <pre class="json">{{ JSON.stringify(snap.snapshot, null, 1) }}</pre>
              </details>
            </div>
            <div v-else class="empty">{{ snapError || 'Nothing uploaded by this player\'s game yet (0.4.3 servers keep it).' }}</div>
          </section>
        </template>
      </div>

      <aside class="col side">
        <section class="panel">
          <span class="lbl">At a glance</span>
          <dl class="kv">
            <dt>Played</dt><dd>{{ fmt.dur(p.play_seconds) }} · {{ fmt.n(p.matches) }} matches</dd>
            <dt>Last 7 days</dt><dd>{{ fmt.dur(p.week_seconds) }}</dd>
            <dt>Sessions</dt><dd>{{ fmt.n(p.sessions) }}</dd>
            <dt>Last seen</dt><dd>{{ p.online ? 'now' : fmt.ago(p.last_seen) }}</dd>
            <dt>Problems</dt><dd>{{ act5 ? `${problems.filter(x => x.level === 'bad').length} in 5 days` : '–' }}</dd>
          </dl>
        </section>
        <section class="panel">
          <span class="lbl">Accounts</span>
          <div class="accounts">
            <div class="acct"><span>{{ serverLabel }}</span><span class="faint mono">#{{ p.id }}{{ p.created_at ? ` · since ${month(p.created_at)}` : '' }}</span></div>
            <RouterLink v-for="o in d.others" :key="o.server + o.id" class="acct" :to="`/players/${encodeURIComponent(o.server)}/${o.id}`">
              <span>{{ o.name }} on {{ names.get(o.server) || o.server }}</span>
              <span class="faint mono">#{{ o.id }}{{ o.banned ? ' · banned' : o.online ? ' · online' : '' }}</span>
            </RouterLink>
            <p class="small faint">{{ d.others.length ? 'One person: the same identity on each. A network ban holds on every server.' : p.identity ? 'No other accounts with this identity.' : 'No identity: older launchers don\'t send one.' }}</p>
          </div>
        </section>
      </aside>
    </div>
  </template>
</template>

<script setup>
// One player's profile (#/players/<server>/<id>): who they are and their state at a glance,
// what admins can do, and tabs for their activity (the Sessions page's timeline, the last
// 5 days), sessions, moderation, fair play and loadouts.
import { computed, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import LineChart from '../components/LineChart.vue';
import PlayerAction from '../components/PlayerAction.vue';
import TempPassword from '../components/TempPassword.vue';
import { api } from '../lib/api.js';
import { ACTION_LABELS, follow, following } from '../lib/actions.js';
import { ensureOverview } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { features } from '../lib/features.js';
import { fmt, PALETTE, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { endText, markClass, stayTitle } from '../lib/stays.js';
import { openModal, toast } from '../lib/ui.js';

const route = useRoute();
const router = useRouter();
onMounted(() => ensureOverview().catch(() => {}));
const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));
const server = computed(() => String(route.params.server || ''));
const id = computed(() => Number(route.params.id));
const serverLabel = computed(() => names.value.get(server.value) || server.value);

const TABS = [['activity', 'Activity'], ['sessions', 'Sessions'], ['moderation', 'Moderation'], ['fair', 'Fair play'], ['loadouts', 'Loadouts']];
const tab = computed(() => (TABS.some(([t]) => t === route.query.tab) ? route.query.tab : 'activity'));
const setTab = t => router.replace({ query: t === 'activity' ? {} : { tab: t } });

const d = ref(null);
const error = ref('');
const p = computed(() => d.value?.player);
const snap = ref(null);
const snapError = ref('');
const support = ref(null);
// Actions taken here, while they're followed.
const mineIds = ref([]);
const mine = computed(() => Object.fromEntries(mineIds.value.filter(aid => following[aid]).map(aid => [aid, following[aid]])));

async function load() {
  const base = `/players/${encodeURIComponent(server.value)}/${id.value}`;
  try {
    d.value = await api('GET', base);
    error.value = '';
  } catch (e) {
    error.value = e.status === 404 ? 'This player is gone (deleted on their server).' : e.message;
    d.value = null;
    return;
  }
  api('GET', `${base}/content`).then(s => { snap.value = s; snapError.value = ''; }).catch(e => { snap.value = null; snapError.value = e.status === 404 ? '' : e.message; });
  // Their support conversation, where the coordinator keeps them.
  if (features.roadmap && p.value.identity) {
    api('GET', `/support/${p.value.identity}`).then(t => { support.value = t; }).catch(() => { support.value = null; });
  } else {
    support.value = null;
  }
}

// The last 5 days of their activity, from the Sessions page's report.
const act5 = ref(null);
const act5Error = ref('');
async function loadActivity() {
  const to = Math.floor(Date.now() / 1000);
  const q = new URLSearchParams({ server: server.value, player: String(id.value), from: String(to - 5 * 86400), to: String(to) });
  try {
    act5.value = await api('GET', `/sessions?${q}`);
    act5Error.value = '';
  } catch (e) {
    act5Error.value = e.message;
  }
}
watch([server, id], () => {
  d.value = null;
  act5.value = null;
  mineIds.value = [];
  load();
  loadActivity();
}, { immediate: true });

const flagsOf = outcome => (d.value?.hardware?.matches || []).filter(m => outcome.includes(m.outcome));
const evasions = computed(() => flagsOf(['evasion']));
const hwMatches = computed(() => flagsOf(['ban', 'flag']));
const flags = computed(() => [...evasions.value, ...hwMatches.value, ...(d.value?.hardware?.vm ? ['vm'] : [])]);

const tabs = computed(() => TABS.map(([t, label]) => [t, label, {
  activity: problems.value.filter(x => x.level === 'bad').length,
  moderation: (d.value?.mutes.length || 0) + (d.value?.network_ban || p.value?.banned ? 1 : 0),
  fair: flags.value.length,
}[t] || 0]));

// The timeline: a row a day, midnight to midnight in the admin's time zone.
const mine5 = computed(() => (act5.value?.players || []).filter(x => x.player === id.value));
// What went wrong: the Sessions report's problems, and their game's crashes and freezes (marks
// on the timeline; the Crashes page groups them).
const problems = computed(() => {
  const found = (act5.value?.problems || []).filter(x => x.player === id.value);
  const crashes = (act5.value?.players || []).filter(x => x.player === id.value).flatMap(x => x.marks)
    .filter(m => m.kind === 'crash')
    .map(m => ({ at: m.at, level: m.level || 'bad', title: /froze/.test(m.text) ? 'Their game froze' : 'Their game crashed', text: m.text }));
  return [...found, ...crashes].sort((a, b) => b.at - a.at);
});
const days = computed(() => {
  const out = [];
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const t = mine5.value;
  for (let i = 0; i < 5; i++) {
    const startDate = new Date(today);
    startDate.setDate(today.getDate() - i);
    const endDate = new Date(startDate);
    endDate.setDate(startDate.getDate() + 1);
    const start = startDate.getTime() / 1000;
    const end = endDate.getTime() / 1000;
    const pos = (a, b) => {
      const l = Math.max(a, start), r = Math.min(b ?? Date.now() / 1000, end);
      return r <= l ? null : { left: `${((l - start) / (end - start)) * 100}%`, width: `${Math.max(0.3, ((r - l) / (end - start)) * 100)}%` };
    };
    out.push({
      start,
      label: i === 0 ? 'Today' : startDate.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' }),
      online: t.flatMap(x => x.online).map(([a, b]) => ({ style: pos(a, b), tip: `Online ${when(a)}–${b ? when(b) : 'now'}` })).filter(o => o.style),
      rooms: t.flatMap(x => x.rooms).map(r => ({
        style: pos(r.from, r.to),
        cls: r.kind !== 'match' ? 'party' : [r.mode || 'coop', r.with.length ? '' : 'alone'],
        tip: `${stayTitle(act5.value.labels, r)}\n${when(r.from)}–${when(r.to)} · ${endText(r)}`,
      })).filter(r => r.style),
      marks: t.flatMap(x => x.marks).filter(m => m.at >= start && m.at < end && ['bad', 'warn'].includes(markClass(m))).map(m => ({
        style: { left: `${((m.at - start) / (end - start)) * 100}%` }, cls: markClass(m), tip: `${when(m.at)} ${m.text}`,
      })),
    });
  }
  return out;
});
const day = t => new Date(t * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });
const month = t => new Date(t * 1000).toLocaleDateString(undefined, { month: 'short', year: 'numeric' });
const when = t => new Date(t * 1000).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });

const daySeries = computed(() => [{ name: 'Played', color: PALETTE[0], points: d.value.days.map(x => ({ t: x.t, v: x.seconds / 60 })) }]);

// Chat mutes: where, and lifting one (wants a second factor proved lately).
const ROOM = { global: 'Global', direct: 'direct messages', lobby: 'lobbies' };
const muteWhere = room => (room === '*' ? 'everywhere' : `in ${ROOM[room] || room}`);
async function unmute(mu) {
  if (!await confirmBox(`Lift ${p.value.name}'s mute?`, `Muted ${muteWhere(mu.room)}: they can post there again at once.`, 'Lift')) return;
  try {
    await api('DELETE', `/moderation/mutes/${encodeURIComponent(d.value.mutes_identity)}/${encodeURIComponent(mu.room)}`);
    toast(`Lifted ${p.value.name}'s mute.`);
    load();
  } catch (e) { toast(e.message, true); }
}

/// A developer: their identity's other builds are taken on every server (clients.rs).
async function allowBuilds() {
  if (!await confirmBox(`Allow ${p.value.name} other builds?`, 'Every server takes their identity\'s modified or development launcher and add-on, even when other builds are refused. Undo in Security › Client builds.', 'Allow')) return;
  try {
    await api('POST', '/clients/allowed', { identity: p.value.identity, note: `${p.value.name}` });
    toast(`${p.value.name} may use other builds: servers take it within a minute.`);
  } catch (e) { toast(e.message, true); }
}

const more = ref(null);
const closeMore = () => { if (more.value) more.value.open = false; };

async function act(kind) {
  let queued;
  if (kind === 'kick' || kind === 'unban' || kind === 'name_not_allowed' || kind === 'allow_name') {
    const says = {
      kick: 'They\'re signed out, and can sign in again.',
      unban: d.value.network_ban ? 'Every account of theirs, on every server, can sign in again.' : 'They can sign in again.',
      name_not_allowed: 'Their name breaks the rules: the launcher and the game\'s overlay ask them to choose another, and the game is kept out until they do. They can ask for a review in Support.',
      allow_name: 'Their name was flagged by mistake: they\'re no longer asked to rename, and the check doesn\'t flag it again. To grant a name the rules refused when they tried it, use Rename.',
    }[kind];
    const ok = await confirmBox(`${ACTION_LABELS[kind]} ${p.value.name}?`, says, ACTION_LABELS[kind]);
    if (!ok) return;
    try {
      queued = (await api('POST', `/players/${encodeURIComponent(server.value)}/${id.value}/actions`, { kind })).actions;
    } catch (e) { toast(e.message, true); return; }
  } else {
    queued = await openModal(PlayerAction, { kind, player: p.value, server: serverLabel.value });
    if (!queued) return;
  }
  const who = p.value.name;
  load();
  for (const a of queued) {
    mineIds.value = [...mineIds.value, a.id];
    follow(a.id, kind).then(done => {
      toast(`${ACTION_LABELS[kind]} ${who}: ${done.message || done.status}`, done.status !== 'done');
      if (done.password) openModal(TempPassword, { who, password: done.password });
      if (kind === 'delete' && done.status === 'done') router.push('/players');
      else load();
    }).catch(e => toast(e.message, true));
  }
}
</script>

<style scoped>
.back { margin-bottom: 10px; font-size: 13px; }
.profile { display: flex; flex-wrap: wrap; gap: 20px; justify-content: space-between; align-items: flex-start; margin-bottom: 14px; }
.who { display: flex; gap: 16px; align-items: flex-start; min-width: 0; }
.who-text { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.who h1 { margin: 0; font-family: var(--display, var(--sans)); font-size: 26px; font-weight: 700; letter-spacing: 0.01em; text-transform: none; word-break: break-word; }
.avatar.big { flex: none; width: 56px; height: 56px; border-radius: 50%; display: grid; place-items: center; font-size: 18px; font-weight: 600; background: var(--chip); color: var(--muted); text-transform: uppercase; }
.avatar.big.on { background: var(--info-soft, var(--chip)); color: var(--info); }
.pills { gap: 6px; flex-wrap: wrap; }
.pill .dot { width: 7px; height: 7px; }
.faint { color: var(--faint); }
.actbar { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.ptabs { display: flex; gap: 2px; border-bottom: 1px solid var(--line); margin-bottom: 16px; flex-wrap: wrap; }
.tab { border: none; border-radius: 0; background: none; color: var(--muted); padding: 10px 12px; height: auto; border-bottom: 2px solid transparent; margin-bottom: -1px; font-size: 13.5px; }
.tab:hover { color: var(--text); background: none; }
.tab[aria-current="page"] { color: var(--text); border-bottom-color: var(--accent); font-weight: 500; }
.tab .count { margin-left: 6px; }
.layout { display: flex; flex-wrap: wrap; gap: 16px; align-items: flex-start; }
.col { flex: 999 1 600px; min-width: 0; display: flex; flex-direction: column; gap: 16px; }
.col.side { flex: 1 1 280px; max-width: 360px; }
.lbl { font-family: var(--mono); font-size: 10.5px; letter-spacing: 0.16em; text-transform: uppercase; color: var(--faint); }
.kv { display: grid; grid-template-columns: 100px 1fr; gap: 8px 12px; font-size: 13px; margin: 10px 0 0; }
.kv dt { color: var(--faint); }
.kv dd { margin: 0; }
.accounts { display: flex; flex-direction: column; gap: 8px; margin-top: 10px; font-size: 13px; }
.acct { display: flex; justify-content: space-between; gap: 8px; color: var(--text); text-decoration: none; }
a.acct:hover span:first-child { color: var(--accent); }
.accounts p { margin: 0; }
.rows { list-style: none; margin: 0; padding: 0; }
.rows li { display: flex; gap: 12px; align-items: flex-start; padding: 9px 0; border-bottom: 1px solid var(--line-soft); }
.rows li:last-child { border-bottom: none; }
.grow { flex: 1; min-width: 0; }
.nowrap { white-space: nowrap; }
.legend { display: flex; flex-wrap: wrap; gap: 12px; font-size: 12px; color: var(--muted); }
.legend span { display: inline-flex; align-items: center; gap: 6px; }
.sw { width: 12px; height: 12px; border-radius: 3px; display: inline-block; }
.sw.online { background: var(--line-2); }
.sw.svm { background: var(--accent); }
.sw.coop { background: var(--info); }
.sw.party { border: 1px solid var(--muted); }
.tick { width: 10px; height: 10px; border-radius: 50%; display: inline-block; }
.tick.bad { background: var(--bad); }
.tick.warn { background: var(--warn); }
.hours { display: grid; grid-template-columns: repeat(6, minmax(0, 1fr)); font: 11px var(--mono); color: var(--faint); margin-left: 104px; }
.day { display: grid; grid-template-columns: 90px 1fr; gap: 14px; align-items: center; padding: 8px 0; border-bottom: 1px solid var(--line-soft); }
.day:last-child { border-bottom: none; }
.track { position: relative; height: 28px; background: var(--panel-2); border: 1px solid var(--line-soft); border-radius: 6px; }
.seg { position: absolute; }
.seg.online { top: 11px; height: 4px; border-radius: 2px; background: var(--line-2); }
.seg.room { top: 5px; height: 16px; border-radius: 4px; }
.seg.svm { background: var(--accent); opacity: 0.75; }
.seg.coop { background: var(--info); opacity: 0.75; }
.seg.party { border: 1px solid var(--muted); }
.seg.alone { opacity: 0.35; }
.mark { position: absolute; top: -4px; width: 11px; height: 11px; border-radius: 50%; border: 2px solid var(--bg); transform: translateX(-5px); }
.mark.bad { background: var(--bad); }
.mark.warn { background: var(--warn); }
.json { max-height: 320px; overflow: auto; font-size: 11px; background: var(--panel-2); padding: 8px; border-radius: 6px; }
.stack { display: flex; flex-direction: column; gap: 8px; }
.stack .callout { margin: 0; }
@media (max-width: 700px) {
  .hours { margin-left: 0; }
  .day { grid-template-columns: 1fr; gap: 4px; }
  .col.side { max-width: none; }
}
</style>
