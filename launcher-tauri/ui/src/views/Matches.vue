<template>
  <div class="matches">
    <header class="head">
      <div class="head-text">
        <p class="kicker">{{ info.server || 'No server yet' }}</p>
        <h1 class="title">Matches</h1>
        <p class="soft small">{{ data?.summary || (data ? '' : 'Reading the open matches…') }}<span v-if="data?.summary"> · updated live</span></p>
      </div>
      <div class="head-act">
        <button type="button" class="btn ghost" :disabled="!ready" @click="newMatch">New match</button>
        <button type="button" class="btn primary" :disabled="!ready" @click="quick">Quick match</button>
      </div>
    </header>

    <p v-if="!info.game_dir || !info.server" class="muted pad">Set up the game and a server first (Play).</p>
    <p v-else-if="data?.error" class="bad small pad" role="alert">Couldn't read the matches: {{ data.error }} <a href="#" @click.prevent="load">Try again</a></p>
    <template v-else-if="data">
      <div v-if="rows.length" class="filters">
        <div class="seg" role="group" aria-label="Mode">
          <button v-for="m in modeFilters" :key="m.id" type="button" :aria-pressed="mode === m.id" @click="mode = m.id">{{ m.label }} · {{ m.count }}</button>
        </div>
        <label class="check"><input v-model="showUnderWay" type="checkbox" /> Under way</label>
        <label class="check"><input v-model="showFull" type="checkbox" /> Full</label>
        <label class="check"><input v-model="friendsFirst" type="checkbox" /> Friends first</label>
        <label class="sort">Sort
          <select v-model="sort" aria-label="Sort the matches">
            <option value="near">Nearest first</option>
            <option value="busy">Most players</option>
            <option value="new">Newest</option>
          </select>
        </label>
      </div>

      <div v-if="!shown.length" class="empty">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M3 9h18M8 13h8M8 16h5" /></svg>
        <h2>{{ rows.length ? 'Nothing matches the filters' : 'No public matches open' }}</h2>
        <p class="soft">{{ rows.length ? 'Show the ones under way or full, or another mode.' : 'Host one and it shows here for everyone; quick match finds you one as soon as anyone opens.' }}</p>
        <div class="row">
          <button type="button" class="btn primary" :disabled="!ready" @click="newMatch">Host a match</button>
          <button type="button" class="btn ghost" @click="$emit('go', 'servers')">See other servers</button>
        </div>
        <p v-if="data.hidden" class="muted small">{{ data.hidden }}</p>
      </div>

      <div v-else class="body">
        <div class="list">
          <div class="cols caps" aria-hidden="true"><span>Match</span><span>Players</span><span>Status</span><span class="r">Ping</span></div>
          <ul class="rows" aria-label="Public matches">
            <li v-for="l in shown" :key="l.room">
              <button type="button" class="match" :class="{ on: picked === l.room }" :aria-pressed="picked === l.room" @click="picked = l.room">
                <span class="what">
                  <b>{{ l.mode }}<template v-if="l.map"> · {{ l.map }}</template></b>
                  <small class="soft">
                    <template v-if="l.host">Hosted by {{ l.host }}</template>
                    <span v-if="l.friends.length" class="friend"> · {{ friendLine(l) }}</span>
                    <span v-if="l.made_for" class="warn"> · {{ l.seats }} seats (made for {{ l.made_for }})</span>
                  </small>
                </span>
                <span class="fill">
                  <b>{{ l.players }}</b><span class="muted">{{ l.seats ? `/${l.seats}` : '' }}</span>
                  <span class="bar" aria-hidden="true"><i :class="{ warn: l.full }" :style="{ width: `${fillOf(l)}%` }"></i></span>
                </span>
                <span class="status"><i class="dot" :class="l.tone" aria-hidden="true"></i>{{ l.status }}</span>
                <span class="ping r" :class="pingTone(l.ping_ms)">{{ l.ping_ms ? `${l.ping_ms} ms` : '–' }}</span>
              </button>
            </li>
          </ul>
          <p v-if="data.hidden" class="muted small foot">{{ data.hidden }} Private matches aren't listed.</p>
        </div>

        <aside v-if="sel" class="detail" :aria-label="`${sel.map || sel.mode}, hosted by ${sel.host}`">
          <div class="band" :class="sel.kind">
            <p class="kicker" :class="{ warnk: sel.tone === 'warn' }">{{ sel.mode }} · public<template v-if="sel.full"> · full</template></p>
            <h2>{{ sel.map || sel.mode }}</h2>
          </div>
          <div class="detail-body" tabindex="0" aria-label="Who's in it and its settings">
            <div class="state" :class="sel.tone">
              <i class="dot big" :class="sel.tone" aria-hidden="true"></i>
              <div>
                <b>{{ sel.status }}</b>
                <small class="soft">Open {{ sel.open_for }}<template v-if="!sel.under_way && !sel.full"> · joinable now</template></small>
              </div>
              <span class="ping" :class="pingTone(sel.ping_ms)">{{ sel.ping_ms ? `${sel.ping_ms} ms` : '' }}</span>
            </div>

            <div v-if="sel.teams" class="teams">
              <section v-for="side in ['spy', 'merc']" :key="side" :aria-labelledby="`side-${side}`">
                <h3 :id="`side-${side}`" class="caps">{{ side === 'spy' ? 'Spies' : 'Mercs' }} · {{ sel.people.filter((p) => p.team === side).length }}</h3>
                <ul class="people">
                  <li v-for="p in sel.people.filter((q) => q.team === side)" :key="p.name" :class="{ friend: p.friend }">
                    <span class="avatar" aria-hidden="true">{{ initial(p.name) }}</span>
                    <span class="who"><b :title="p.name">{{ p.name }}</b><span v-if="p.host" class="tag">HOST</span><small v-if="p.friend" class="ok">friend</small></span>
                  </li>
                </ul>
              </section>
            </div>
            <section v-else aria-labelledby="in-lobby">
              <h3 id="in-lobby" class="caps">In this {{ sel.under_way ? 'match' : 'lobby' }} · {{ sel.players }}<template v-if="sel.seats"> of {{ sel.seats }}</template></h3>
              <ul class="people">
                <li v-for="p in sel.people" :key="p.name" :class="{ friend: p.friend, party: p.party }">
                  <span class="avatar" aria-hidden="true">{{ initial(p.name) }}</span>
                  <span class="who">
                    <b :title="p.name">{{ p.name }}</b>
                    <span v-if="p.host" class="tag">HOST</span>
                    <small v-if="p.friend" class="ok">friend</small>
                    <small v-else-if="p.party" class="muted">{{ partyLine(sel, p) }}</small>
                  </span>
                  <span class="ping" :class="pingTone(p.ping_ms)">{{ p.ping_ms ? `${p.ping_ms} ms` : '' }}</span>
                </li>
                <li v-if="sel.seats > sel.players" class="open-seats">
                  <span class="avatar empty" aria-hidden="true"></span>{{ sel.seats - sel.players }} open {{ sel.seats - sel.players === 1 ? 'seat' : 'seats' }}
                </li>
              </ul>
            </section>

            <section v-if="sel.settings.length" aria-labelledby="settings">
              <h3 id="settings" class="caps">Match settings</h3>
              <dl class="settings">
                <div v-for="[k, v] in sel.settings" :key="k"><dt>{{ k }}</dt><dd>{{ v }}</dd></div>
              </dl>
            </section>
          </div>
          <div class="detail-act">
            <button v-if="sel.joinable" type="button" class="join" :disabled="joining" @click="join(sel)">Join this match</button>
            <button v-else type="button" class="join off" disabled>{{ sel.under_way ? 'Under way' : 'Full' }}<template v-if="sel.under_way && sel.full"> · full</template></button>
            <button v-if="!sel.joinable" type="button" class="btn ghost" :aria-pressed="watching?.room === sel.room" @click="watch(sel)">
              {{ watching?.room === sel.room ? 'Watching: we\'ll tell you when it opens' : 'Tell me when a seat opens' }}
            </button>
            <p v-else class="muted small center">Starts the game if it isn't running, then joins {{ sel.host || 'its host' }}'s {{ sel.under_way ? 'match' : 'lobby' }}.</p>
          </div>
        </aside>
      </div>
    </template>
    <p v-else class="muted working pad"><i class="spin"></i>Reading the open matches…</p>
  </div>
</template>

<script setup>
// The Matches tab (the 0.5.0 lobby browser, its own tab): every public match on the player's
// server in full (who's in it, its state and settings), from the server's Lobbies; picked to
// join, or watched for a seat. A server before 0.5.0 says no more than the open matches.
import { computed, onBeforeUnmount, onMounted, ref, watch as watchRef } from 'vue';
import { call } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true }, watching: { type: Object, default: null } });
const emit = defineEmits(['go', 'watch', 'played']);

const data = ref(null);
const picked = ref(null);
const mode = ref('all');
const showUnderWay = ref(true);
const showFull = ref(false);
const friendsFirst = ref(true);
const sort = ref('near');
const joining = ref(false);

const ready = computed(() => Boolean(props.info.game_dir && props.info.server && !props.info.running));

async function load() {
  let got = await call('lobbies');
  // A server before 0.5.0: the open matches as it lists them, with no one named.
  if (got && !got.offered && !got.error) {
    const old = await call('matches');
    got = {
      offered: false,
      error: null,
      summary: old?.others || '',
      hidden: '',
      lobbies: (old?.rows || []).map((r) => ({
        room: r.room, mode: r.mode, kind: /co-?op/i.test(r.mode) ? 'coop' : 'svm', map: r.map, players: parseInt(r.players, 10) || 0,
        seats: parseInt((r.players.split('/')[1] || '0'), 10), made_for: null, host: '', status: r.started ? 'Under way' : 'In lobby',
        tone: r.started || r.full ? 'warn' : 'ok', under_way: r.started, full: r.full, joinable: !r.started && !r.full, open_for: '',
        ping_ms: null, friends: [], teams: false, people: [], settings: [],
      })),
    };
  }
  data.value = got;
}

const rows = computed(() => data.value?.lobbies || []);
const modeFilters = computed(() => [
  { id: 'all', label: 'All', count: rows.value.length },
  { id: 'svm', label: 'Spies vs Mercs', count: rows.value.filter((l) => l.kind !== 'coop').length },
  { id: 'coop', label: 'Co-op', count: rows.value.filter((l) => l.kind === 'coop').length },
]);
const shown = computed(() => {
  const list = rows.value.filter(
    (l) =>
      (mode.value === 'all' || (mode.value === 'coop') === (l.kind === 'coop')) &&
      (showUnderWay.value || !l.under_way) &&
      // A full match under way is the Under way filter's.
      (showFull.value || !l.full || l.under_way || l.room === picked.value),
  );
  const key = {
    near: (l) => l.ping_ms ?? 9999,
    busy: (l) => -l.players,
    new: (l) => l.open_secs ?? 0,
  }[sort.value];
  return list.sort((a, b) => (friendsFirst.value ? (b.friends.length > 0) - (a.friends.length > 0) : 0) || (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));
});
const sel = computed(() => shown.value.find((l) => l.room === picked.value) || null);
// The first one picked once the list comes, and again when the picked one goes.
watchRef(shown, (list) => {
  if (!list.some((l) => l.room === picked.value)) picked.value = list[0]?.room ?? null;
});

function fillOf(l) {
  return l.seats ? Math.min(100, Math.round((100 * l.players) / l.seats)) : 0;
}
function pingTone(ms) {
  return !ms ? 'muted' : ms <= 120 ? 'ok' : 'warn';
}
function initial(name) {
  return (name || '?').charAt(0).toUpperCase();
}
function friendLine(l) {
  const [first, ...rest] = l.friends;
  return rest.length ? `${first} and ${rest.length} more of your friends are in it` : `${first}, your friend, is in it`;
}
function partyLine(l, p) {
  const others = l.people.filter((q) => q.party === p.party && q.name !== p.name).map((q) => q.name);
  return others.length ? `party with ${others.join(', ')}` : 'in a party';
}

async function join(l) {
  joining.value = true;
  await call('join_match', { room: l.room });
  joining.value = false;
  emit('played');
}
function newMatch() {
  call('new_match', { mode: mode.value === 'coop' ? 'coop' : 'svm' });
  emit('played');
}
async function quick() {
  const started = await call('play', { mode: mode.value === 'coop' ? 'coop' : 'svm' });
  emit('played', started);
}
function watch(l) {
  emit('watch', props.watching?.room === l.room ? null : { room: l.room, label: `${l.mode}${l.map ? ` · ${l.map}` : ''}` });
}

let timer = null;
onMounted(() => {
  load();
  timer = setInterval(load, 5000);
});
onBeforeUnmount(() => clearInterval(timer));
</script>

<style scoped>
.matches { height: 100%; display: flex; flex-direction: column; min-width: 0; }
.head { display: flex; align-items: flex-end; gap: 24px; padding: 26px 32px 14px; border-bottom: 1px solid var(--line); flex-wrap: wrap; }
.head-text { flex: 1 1 320px; min-width: 0; }
.head .title { margin-bottom: 4px; }
.head-act { display: flex; gap: 10px; }
.head-act .btn { height: 44px; padding: 0 18px; }
.pad { padding: 20px 32px; }
.filters { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 12px 32px; border-bottom: 1px solid var(--line); }
.seg { display: flex; border: 1px solid var(--control-line); border-radius: 4px; overflow: hidden; }
.seg button { height: 34px; padding: 0 14px; background: var(--surface); color: var(--soft); font-size: 13px; }
.seg button + button { border-left: 1px solid var(--control-line); }
.seg button[aria-pressed='true'] { background: var(--selected); color: var(--accent); font-weight: 600; }
.check { display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 12px; border: 1px solid var(--control-line); border-radius: 4px; background: var(--surface); color: var(--soft); font-size: 13px; }
.check input { accent-color: var(--accent); }
.sort { margin-left: auto; display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 13px; }
.sort select { height: 34px; padding: 0 10px; border: 1px solid var(--control-line); border-radius: 4px; background: var(--surface); color: var(--fg); font: 13px Plex, sans-serif; }
.body { flex: 1; min-height: 0; display: flex; }
.list { flex: 1; min-width: 0; overflow: auto; padding: 6px 0 16px; }
.cols, .match { display: grid; grid-template-columns: minmax(0, 1.6fr) 110px minmax(0, 1.2fr) 70px; gap: 12px; align-items: center; padding: 0 32px; }
.cols { padding-top: 8px; padding-bottom: 8px; font-size: 11px; }
.r { text-align: right; }
.rows { list-style: none; margin: 0; padding: 0; }
.rows li + li .match { border-top: 1px solid var(--line); }
.match { width: 100%; text-align: left; padding-top: 12px; padding-bottom: 12px; transition: background .15s; }
.match:hover { background: var(--control); }
.match.on { background: var(--selected); box-shadow: inset 3px 0 0 var(--accent); }
.match:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.what { min-width: 0; }
.what b { display: block; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.what small { display: block; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.friend, .ok { color: var(--accent); }
.warn { color: var(--warn); }
.bad { color: var(--bad); }
.bar { display: block; height: 4px; margin-top: 5px; border-radius: 2px; background: var(--line); }
.bar i { display: block; height: 4px; border-radius: 2px; background: var(--accent); }
.bar i.warn { background: var(--warn); }
.status { display: flex; align-items: center; gap: 8px; font-size: 14px; min-width: 0; }
.dot { flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--accent); }
.dot.warn { background: var(--warn); }
.dot.big { width: 10px; height: 10px; box-shadow: 0 0 0 4px rgba(155, 224, 90, .15); }
.dot.big.warn { box-shadow: 0 0 0 4px rgba(240, 180, 76, .15); }
.ping { font: 14px PlexMono, monospace; }
.ping.ok { color: var(--accent); }
.ping.warn { color: var(--warn); }
.ping.muted { color: var(--muted); }
.foot { margin: 10px 32px 0; }
.detail { width: 400px; flex: none; border-left: 1px solid var(--line); background: var(--surface); display: flex; flex-direction: column; min-height: 0; }
.band { height: 112px; padding: 0 22px 12px; display: flex; flex-direction: column; justify-content: flex-end; background: linear-gradient(180deg, rgba(12, 17, 18, 0) 20%, var(--surface) 100%), repeating-linear-gradient(135deg, #142019 0 14px, #101a15 14px 28px); }
.band.coop { background: linear-gradient(180deg, rgba(12, 17, 18, 0) 20%, var(--surface) 100%), repeating-linear-gradient(135deg, #12192a 0 14px, #0f1522 14px 28px); }
.band .kicker.warnk { color: var(--warn); }
.band h2 { margin: 2px 0 0; font: 700 30px/1 PlexCond, sans-serif; text-transform: uppercase; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.detail-body:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.detail-body { flex: 1; min-height: 0; overflow: auto; padding: 14px 22px; display: flex; flex-direction: column; gap: 16px; }
.state { display: flex; align-items: center; gap: 10px; padding: 10px 12px; border: 1px solid var(--control-line); border-radius: 6px; background: var(--sunken); }
.state.warn { border-color: #3a3020; }
.state > div { flex: 1; min-width: 0; }
.state b { display: block; font-size: 14px; }
.state small { font-size: 12.5px; }
.caps { margin: 0 0 6px; }
.teams { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.people { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.people li { display: flex; align-items: center; gap: 10px; padding: 6px 8px; border-radius: 4px; }
.people li.friend { background: var(--selected); }
.people li.party { border-left: 2px solid var(--control-line); margin-left: 4px; }
.avatar { flex: none; width: 28px; height: 28px; border-radius: 50%; display: grid; place-items: center; font-size: 12px; font-weight: 600; background: var(--line); }
.people li.friend .avatar { background: #1f3524; color: var(--accent); }
.avatar.empty { background: none; border: 1px dashed var(--control-line); }
.who { flex: 1; min-width: 0; display: flex; align-items: baseline; gap: 6px; overflow: hidden; }
.who b { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.who small { font-size: 12.5px; white-space: nowrap; }
.teams .who { flex-wrap: wrap; row-gap: 0; }
.tag { font: 10.5px PlexMono, monospace; letter-spacing: .08em; color: var(--accent); border: 1px solid rgba(155, 224, 90, .4); border-radius: 3px; padding: 0 5px; }
.people .ping { font-size: 12.5px; }
.open-seats { color: var(--muted); font-size: 13px; }
.settings { margin: 0; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px 16px; font-size: 13px; }
.settings div { display: flex; justify-content: space-between; gap: 8px; }
.settings dt { color: var(--soft); }
.settings dd { margin: 0; }
.detail-act { padding: 14px 22px 18px; border-top: 1px solid var(--line); display: flex; flex-direction: column; gap: 8px; }
.join { height: 48px; border-radius: 4px; background: var(--accent); color: var(--on-accent); font: 700 18px PlexCond, sans-serif; letter-spacing: .12em; text-transform: uppercase; }
.join:hover:not(:disabled) { filter: brightness(1.08); }
.join.off { background: var(--control); color: var(--muted); border: 1px solid var(--control-line); cursor: not-allowed; }
.detail-act .btn { height: 40px; }
.center { text-align: center; margin: 0; }
.empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; gap: 12px; padding: 24px; color: var(--muted); }
.empty h2 { margin: 0; font: 700 24px/1.1 PlexCond, sans-serif; text-transform: uppercase; color: var(--fg); }
.empty p { max-width: 440px; margin: 0; }
.empty .row { display: flex; gap: 10px; flex-wrap: wrap; justify-content: center; }
.empty .btn { height: 44px; padding: 0 18px; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: turn .8s linear infinite; }
@keyframes turn { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .spin { animation-duration: 2.4s; } }
/* A short window: the header and the map band take less, so the players stay in view. */
@media (max-height: 720px) {
  .head { padding-top: 14px; padding-bottom: 10px; }
  .head .title { font-size: 30px; margin: 0 0 2px; }
  .head-act .btn { height: 40px; }
  .filters { padding-top: 8px; padding-bottom: 8px; }
  .band { height: 64px; padding-bottom: 8px; }
  .band h2 { font-size: 22px; }
  .detail-body { gap: 10px; padding-top: 10px; }
  .detail-act { padding-top: 10px; padding-bottom: 12px; }
  .detail-act .center { display: none; }
}
/* A smaller window keeps the same two panes: the details narrower, the list's columns closer. */
@media (max-width: 1120px) {
  .head, .filters { padding-left: 22px; padding-right: 22px; }
  .cols, .match { grid-template-columns: minmax(0, 1fr) 76px minmax(0, 1fr) 60px; gap: 10px; padding-left: 22px; padding-right: 22px; }
  .detail { width: 320px; }
  .band { height: 88px; }
  .band h2 { font-size: 24px; }
  .teams { grid-template-columns: minmax(0, 1fr); }
}
</style>
