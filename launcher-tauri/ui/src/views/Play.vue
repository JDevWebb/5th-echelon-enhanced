<template>
  <div class="play-screen">
    <section class="stage">
      <Transition name="art">
        <div v-if="art" class="art" :style="{ backgroundImage: `url(${art})` }" aria-hidden="true"></div>
      </Transition>
      <div class="scan" aria-hidden="true"></div>
      <div class="glow" aria-hidden="true"></div>
      <div v-if="info.name_not_allowed || (info.emulator_notice && !emulatorClosed)" class="banners">
      <div v-if="info.name_not_allowed" class="banner bad" role="alert">
        <span>Your name {{ info.name_not_allowed }} breaks this network's rules (no slurs or hate). Choose another to play: friends and invites carry on.</span>
        <button type="button" class="btn small" @click="$emit('go', 'settings', 'servers')">Rename…</button>
        <!-- Support is the community network's: elsewhere, the server's own admins. -->
        <button v-if="info.community" type="button" class="btn small" title="Think it's a mistake? The admins check the name and can allow it" @click="askReview">Ask for a review</button>
      </div>
      <div v-if="info.emulator_notice && !emulatorClosed" class="banner warnbg" role="status">
        <span>This game's Ubisoft Connect DLL has been replaced by an emulator. That's up to you and we don't mind, but it isn't a setup we support: if something doesn't work with it, we may not be able to help.</span>
        <button type="button" class="x" aria-label="Dismiss" title="Dismiss: don't show this again" @click="closeEmulator"><svg viewBox="0 0 12 12"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" /></svg></button>
      </div>
      </div>
      <div class="hud">
        <span class="caps">{{ inTauri ? 'Field ops' : 'Browser preview' }}</span>
        <button class="player" type="button" title="Identity and friends" @click="$emit('go', 'settings', 'identity')">
          <span class="avatar">{{ (info.account || '?')[0].toUpperCase() }}<i :class="{ on: online }"></i></span>
          <span><b>{{ info.account || 'No account yet' }}</b><small>{{ info.identity ? `ID ${info.identity}` : 'No identity yet' }}</small></span>
        </button>
      </div>
      <div class="hero">
        <p class="kicker reveal" style="--d: 0ms">Tom Clancy's</p>
        <h1 class="reveal" style="--d: 80ms">Splinter Cell<br /><span>Blacklist</span></h1>
        <p class="tag reveal" style="--d: 180ms">Co-op and Spies vs Mercs on community servers. No VPN, no port forwarding.</p>
        <div class="launch reveal" style="--d: 260ms">
          <span class="server-wrap">
            <button
              ref="serverButton"
              class="server"
              :class="{ open: serverMenu }"
              type="button"
              aria-haspopup="menu"
              :aria-expanded="serverMenu"
              :title="serverBlocked || null"
              @click.stop="openServerMenu"
              @keydown.down.prevent="openServerMenu"
            >
              <span class="who">
                <span class="caps" :title="servers?.card?.caption_why || null">{{ servers?.card?.caption ?? 'Server' }}</span>
                <b>{{ servers?.card?.place ?? info.server ?? 'None chosen' }}</b>
              </span>
              <span v-if="servers?.card?.ping !== undefined && servers?.card?.players != null" class="ping">
                <span :class="pingClass(servers.card.ping)">{{ servers.card.ping == null ? 'no answer' : `${servers.card.ping} ms` }}</span>
                <small>{{ servers.card.players }} online</small>
              </span>
              <svg class="chev" viewBox="0 0 24 24"><path d="M6 9l6 6 6-6" /></svg>
            </button>
            <Transition name="menu">
              <div v-if="serverMenu" class="server-menu" role="menu" @click.stop @keydown="menuKeys">
                <p class="menu-head" role="none">
                  <span class="caps">{{ servers.header }}</span>
                  <i v-if="pinging" class="spin"></i>
                  <a v-else-if="servers.network" href="#" role="menuitem" @click.prevent="loadServers(true)">Ping again</a>
                </p>
                <p v-if="!servers.rows.length" class="muted" role="none">{{ pinging ? 'Pinging the servers…' : 'This network lists no servers right now.' }}</p>
                <button
                  v-for="(r, i) in servers.rows"
                  :key="r.host"
                  :ref="(el) => (menuItems[i] = el)"
                  type="button"
                  role="menuitemradio"
                  :aria-checked="r.current"
                  :class="{ on: r.current }"
                  :title="r.playing ? `Playing now: ${r.playing}` : null"
                  @click="switchTo(r)"
                >
                  <span class="row-main">
                    <b>{{ r.place }}</b>
                    <small>{{ r.name }} · {{ r.host }}</small>
                  </span>
                  <span class="tags">
                    <span v-if="r.best" class="mtag best">Best for you</span>
                    <span v-else-if="r.current" class="mtag current">Current</span>
                    <span v-if="r.maintenance" class="mtag warn">{{ r.maintenance }}</span>
                  </span>
                  <span class="ping">
                    <span :class="pingClass(r.ping)">{{ r.ping == null ? 'no answer' : `${r.ping} ms` }}</span>
                    <small>{{ r.players }} online</small>
                  </span>
                </button>
                <p v-if="servers.note" class="muted note" role="none">{{ servers.note }}</p>
                <button type="button" class="to-servers" role="menuitem" @click="serverMenu = false; $emit('go', 'servers')">
                  <span class="soft">A server of your own, a LAN server or another network</span>
                  <b>Servers ›</b>
                </button>
              </div>
            </Transition>
          </span>
          <span class="split" :class="button.tone">
            <button class="play" type="button" :disabled="button.disabled" :title="blocked || null" @click="button.act()">
              <span class="label">{{ button.label }}</span>
              <small v-if="button.sub" class="sub">{{ button.sub }}</small>
            </button>
            <button
              ref="modesButton"
              class="play-more"
              type="button"
              aria-label="Play SvM or Co-op"
              aria-haspopup="menu"
              :aria-expanded="menuOpen"
              :disabled="!!blocked || info.managed || !info.game_dir || !info.server"
              :title="info.managed ? 'This game is set up by another tool, so the launcher starts it as it is.' : null"
              @click.stop="toggleModes"
              @keydown.up.prevent="menuOpen || toggleModes()"
            >
              <svg viewBox="0 0 24 24"><path d="M6 15l6-6 6 6" /></svg>
            </button>
            <Transition name="menu">
              <span v-if="menuOpen" class="modes" role="menu" @keydown="modeKeys">
                <button
                  v-for="(mode, i) in MODES"
                  :key="mode"
                  type="button"
                  :ref="(el) => (modeItems[i] = el)"
                  role="menuitemradio"
                  :aria-checked="info.play_mode === mode"
                  :class="{ on: info.play_mode === mode }"
                  @click="choose(mode)"
                >
                  <b>{{ LABELS[mode] }}</b><small>{{ HINTS[mode] }}</small>
                </button>
                <template v-if="chooseOffered">
                  <span class="sep" role="separator"></span>
                  <button
                    v-for="(mode, j) in ['svm', 'coop']"
                    :key="`new-${mode}`"
                    type="button"
                    :ref="(el) => (modeItems[MODES.length + j] = el)"
                    role="menuitem"
                    @click="newMatch(mode)"
                  >
                    <b>New {{ mode === 'svm' ? 'SvM' : 'Co-op' }} match</b><small>Skip the open ones and host</small>
                  </button>
                  <button type="button" :ref="(el) => (modeItems[MODES.length + 2] = el)" role="menuitem" @click="menuOpen = false; picking = true">
                    <b>Choose a match…</b><small>See the open ones and pick</small>
                  </button>
                  <button v-if="info.new_match" type="button" :ref="(el) => (modeItems[MODES.length + 3] = el)" role="menuitem" @click="cancelNewMatch">
                    <b>Join open matches again</b><small>Take back the new match</small>
                  </button>
                </template>
              </span>
            </Transition>
          </span>
        </div>
        <Transition name="view">
          <div v-if="notice" class="notice" role="status">
            <div class="notice-top">
              <div>
                <p class="notice-head"><b>{{ notice.heading }}</b><span class="eta">{{ notice.eta }}</span></p>
                <p class="soft">{{ notice.text }}</p>
              </div>
              <div class="notice-act">
                <small v-if="notice.note" class="muted">{{ notice.note }}</small>
                <button v-if="notice.play_on" type="button" class="btn" :disabled="info.connecting" @click="switchTo({ host: notice.play_on[0], current: false })">{{ notice.play_on[1] }}</button>
                <button v-if="notice.update" type="button" class="btn primary" @click="call('update_asked')">{{ notice.update }}</button>
              </div>
            </div>
            <div v-if="notice.servers.length" class="notice-servers">
              <div v-for="r in notice.servers" :key="r.place" class="nbox" :class="{ mine: r.mine }">
                <p><b>{{ r.mine ? `${r.place} · yours` : r.place }}</b><span class="mono muted">{{ r.version }}</span></p>
                <p class="doing" :class="r.tone">{{ r.doing }}</p>
                <span v-if="r.meter != null" class="meter"><i :style="{ width: `${r.meter * 100}%` }"></i></span>
              </div>
            </div>
          </div>
        </Transition>
        <p v-if="info.beta" class="beta">Beta features on ({{ info.beta }}): experimental, and your game's log is sent to the server's admins after each game. Settings › Game to turn off.</p>
        <div v-if="news.length" class="newsline">
          <span class="caps accent">Intel</span>
          <Transition name="fade" mode="out-in"><span :key="newsAt" class="headline">{{ news[newsAt].title }}</span></Transition>
          <a href="#" @click.prevent="$emit('go', 'intel')">All news</a>
        </div>
      </div>
    </section>

    <aside class="drawer">
      <div class="tabs" role="tablist">
        <button
          v-for="(t, i) in tabs"
          :key="t.id"
          type="button"
          role="tab"
          :aria-selected="tab === t.id"
          :class="{ on: tab === t.id }"
          @click="tab = t.id"
        >
          {{ t.label }}
        </button>
        <span class="underline" :style="{ width: `${100 / tabs.length}%`, transform: `translateX(${tabIndex * 100}%)` }"></span>
      </div>
      <Transition name="view" mode="out-in">
        <section v-if="tab === 'status'" key="status" class="pane status">
          <p v-if="!list" class="muted working"><i class="spin"></i>Checking…</p>
          <ul v-else>
            <li v-for="c in list.checks" :key="c.id" class="check">
              <span class="mark" :class="c.status" aria-hidden="true"></span>
              <span>
                <b>{{ c.title }}</b>
                <small v-if="c.detail">{{ c.detail }}</small>
                <button v-if="c.fix" type="button" class="fixbtn" :disabled="list.busy" @click="runFix(c.fix[0])">{{ c.fix[1] }}</button>
              </span>
            </li>
            <li v-if="list.support" class="check">
              <span class="mark" :class="list.support[0]" aria-hidden="true"></span>
              <span>
                <b>{{ list.support[1] }}</b>
                <small>{{ list.support[2] }}</small>
                <button v-if="list.support[0] === 'fail'" type="button" class="fixbtn" :disabled="list.busy" @click="identify">Try to identify</button>
              </span>
            </li>
          </ul>
          <div v-if="list" class="foot">
            <p class="links">
              <a href="#" @click.prevent="allChecks = !allChecks; loadChecks(false)">{{ allChecks ? 'Show less' : 'All checks' }}</a>
              <span class="muted">·</span>
              <i v-if="list.busy" class="spin"></i>
              <a v-else href="#" @click.prevent="loadChecks(true)">Check again</a>
              <span class="muted">·</span>
              <a href="#" @click.prevent="$emit('go', 'settings', 'network')">Connection test</a>
            </p>
            <button v-if="list.fixable" type="button" class="btn primary" @click="fixEverything">Fix everything</button>
          </div>
        </section>
        <section v-else-if="tab === 'squad'" key="squad" class="pane squad">
          <p v-if="!squad" class="muted working"><i class="spin"></i>Looking for your friends…</p>
          <template v-else>
            <p v-if="squad.managed || (!squad.online.length && !squad.elsewhere.length)" class="soft">
              {{ squad.offline ? 'None of your friends are online right now.' : 'No friends yet: Find players in Friends.' }}
            </p>
            <ul>
              <li v-for="f in squad.online" :key="f.id">
                <span class="edge"></span>
                <span><b>{{ f.name }}</b><small>{{ f.activity || 'Online' }}</small></span>
                <button
                  v-if="squad.join_offered"
                  type="button"
                  class="fixbtn"
                  :disabled="!f.joinable || info.connecting"
                  :title="f.joinable ? 'Join this friend’s game' : f.activity ? `${f.name}’s match can’t be joined now: it’s a private match (you need an invitation), or it has started` : `${f.name} isn’t in a game right now`"
                  @click="joinFriend(f)"
                >Join</button>
              </li>
              <li v-for="f in squad.elsewhere" :key="f.name + f.host">
                <span class="edge busy"></span>
                <span><b>{{ f.name }}</b><small>On {{ f.place }}</small></span>
                <button type="button" class="fixbtn" :disabled="!f.public || info.running || info.connecting" :title="info.running ? 'Quit the game first' : f.host" @click="confirmSwitch = f">Join</button>
              </li>
            </ul>
            <template v-if="squad.others.length && !squad.managed">
              <p class="soft sub">Online now · {{ squad.others.length }}</p>
              <ul>
                <li v-for="p in squad.others.slice(0, 8)" :key="p.id" @contextmenu.prevent="blockMenu = p">
                  <span class="edge"></span>
                  <span><b>{{ p.name }}</b><small>{{ p.activity || 'Online' }}</small></span>
                  <button v-if="info.running" type="button" class="fixbtn" title="To the game you're in" @click="act('invite', p)">Invite</button>
                  <button type="button" class="fixbtn" :disabled="p.asked && !p.asked_us" @click="act('request', p)">{{ p.asked_us ? 'Accept' : p.asked ? 'Asked' : 'Add friend' }}</button>
                </li>
              </ul>
              <p v-if="squad.others.length > 8" class="muted small">and {{ squad.others.length - 8 }} more: Find players in Friends lists them all</p>
              <div v-if="blockMenu" class="block">
                <span>Block {{ blockMenu.name }}? Neither of you can message, invite or friend the other, you don't see each other online, and quick match leaves out matches they're in.</span>
                <span class="row"><button type="button" class="btn small danger" @click="act('block', blockMenu); blockMenu = null">Block</button><button type="button" class="btn small" @click="blockMenu = null">Cancel</button></span>
              </div>
            </template>
            <template v-if="squad.playing">
              <p class="caps sub">Playing now</p>
              <p v-if="!squad.playing.length && !squad.playing_more" class="muted small">Nobody's in a match or a lobby here right now.</p>
              <p v-for="line in squad.playing" :key="line" class="line">{{ line }}</p>
              <p v-if="squad.playing_more" class="muted small">{{ squad.playing_more }}</p>
            </template>
            <div v-if="info.community" class="comms">
              <p class="caps">Global · latest</p>
              <p v-if="latest" class="soft"><b class="accent">{{ latest[0] }}</b> {{ latest[1] }}</p>
              <p v-else class="muted small">Nothing in Global yet.</p>
              <button type="button" class="btn" @click="$emit('go', 'chat')">Open Chat</button>
            </div>
            <div class="foot">
              <p v-if="squad.requests" class="accent small">{{ squad.requests }} friend request{{ squad.requests === 1 ? '' : 's' }} waiting</p>
              <p class="muted small">{{ squad.offline ? `${squad.offline} offline · ` : '' }}<a href="#" class="to-friends" @click.prevent="$emit('go', 'friends')">Friends, requests and blocked players ›</a></p>
            </div>
          </template>
        </section>
        <section v-else key="intel" class="pane intel">
          <p v-if="!news.length" class="soft">No news from this server.</p>
          <template v-else>
            <p class="muted small">{{ newsAt + 1 }} of {{ news.length }}</p>
            <Transition name="view" mode="out-in">
              <article :key="newsAt">
                <h3>{{ news[newsAt].title }}</h3>
                <p class="soft">{{ news[newsAt].text }}</p>
              </article>
            </Transition>
            <div class="foot dots">
              <button type="button" aria-label="Newer" @click="turn(-1)"><svg viewBox="0 0 24 24"><path d="M15 6l-6 6 6 6" /></svg></button>
              <button v-for="(_, i) in news.slice(0, 8)" :key="i" type="button" class="dot" :class="{ on: i === newsAt }" :aria-label="`News ${i + 1}`" @click="newsAt = i; turned = Date.now()"></button>
              <button type="button" aria-label="Older" @click="turn(1)"><svg viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg></button>
              <span class="grow"></span>
              <a href="#" @click.prevent="$emit('go', 'intel')">View all</a>
            </div>
          </template>
        </section>
      </Transition>
    </aside>

    <MatchPicker :open="picking" @close="picking = false" />

    <Transition name="modal">
      <div v-if="confirmSwitch" class="backdrop" @click.self="confirmSwitch = null">
        <div v-modal="() => (confirmSwitch = null)" class="dialog" role="dialog" aria-modal="true" aria-labelledby="switch-title">
          <p class="kicker">Join {{ confirmSwitch.name }}</p>
          <h2 id="switch-title">Switch to {{ confirmSwitch.place }}?</h2>
          <p class="soft">Switch to your friend's server <b class="mono">{{ confirmSwitch.host }}</b>? This signs you in there with your identity (if you have no account there, you choose a name first).</p>
          <div class="row end">
            <button type="button" class="btn" @click="confirmSwitch = null">Cancel</button>
            <button type="button" class="btn primary" @click="switchToFriend">Switch to {{ confirmSwitch.host }}</button>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup>
// The Play screen, the trial's (trials/tauri-play) on the launcher's real data: the game, the
// server and the account, and Play (with its modes) starting the game through the shared
// library. The server menu, friends and the news come next.
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { call, inTauri, listen } from '../lib/tauri.js';
import MatchPicker from '../components/MatchPicker.vue';

// `latest`: the latest in Global (who, what), for the Squad tab on the community network.
const props = defineProps({ info: { type: Object, required: true }, latest: { type: Array, default: null } });
const emit = defineEmits(['go', 'played', 'change-server']);

// What the Play button starts (the shared library's PlayMode).
const MODES = ['play', 'svm', 'coop'];
const LABELS = { play: 'Play', svm: 'Play SvM', coop: 'Play Coop' };
const HINTS = { play: 'The game as it is', svm: 'Straight into a Spies vs Mercs lobby', coop: 'Straight into a co-op lobby' };

const menuOpen = ref(false);
const closeMenu = () => {
  menuOpen.value = false;
  serverMenu.value = false;
};
onMounted(() => document.addEventListener('click', closeMenu));
onBeforeUnmount(() => document.removeEventListener('click', closeMenu));

/** Why Play can't start now, if it can't. */
const blocked = computed(() => {
  if (!props.info.game_dir) return "Choose the game's folder first.";
  if (props.info.running) return 'The game is running.';
  return notice.value?.blocks_play ?? null;
});

async function play(mode) {
  menuOpen.value = false;
  const started = await call('play', { mode });
  emit('played', started);
}

async function choose(mode) {
  await call('set_play_mode', { mode });
  props.info.play_mode = mode;
  play(mode);
}

// Status first when something needs fixing, else the squad.
const trouble = computed(() => list.value?.tone === 'fail');
const tabs = computed(() => [
  { id: 'status', label: trouble.value ? 'Status · fix' : 'Status' },
  { id: 'squad', label: squad.value?.online.length ? `Squad · ${squad.value.online.length}` : 'Squad' },
  { id: 'intel', label: 'Intel' },
]);
const picked = ref(null);
const tab = computed({ get: () => picked.value ?? (trouble.value ? 'status' : 'squad'), set: (t) => (picked.value = t) });
const tabIndex = computed(() => Math.max(0, tabs.value.findIndex((t) => t.id === tab.value)));

// The server card and menu: the network's servers, pinged on the Rust side.
const servers = ref(null);
const serverMenu = ref(false);
const pinging = ref(false);
const menuItems = ref([]);

async function loadServers(fresh) {
  pinging.value = true;
  servers.value = await call('servers', { fresh });
  pinging.value = false;
}

/** Why the server can't be changed now, if it can't. */
const serverBlocked = computed(() => {
  if (props.info.running) return 'Quit the game first';
  if (props.info.connecting || list.value?.busy) return 'Wait for the current task to finish';
  if (props.info.managed) return 'Another tool manages this install: change the server there';
  return null;
});

function openServerMenu() {
  if (serverBlocked.value) return;
  menuOpen.value = false;
  serverMenu.value = !serverMenu.value;
  if (serverMenu.value) {
    if (!servers.value) loadServers(false);
    // The arrow keys start on the player's server.
    nextTick(() => {
      const at = Math.max(0, servers.value?.rows.findIndex((r) => r.current) ?? 0);
      menuItems.value[at]?.focus();
    });
  }
}

/** A menu's keys: the arrows move between its items, Escape closes it back to its button. */
function keysIn(e, items, closed, button) {
  items = items.filter(Boolean);
  const at = items.indexOf(document.activeElement);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    items[(at + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
  } else if (e.key === 'Escape') {
    e.preventDefault();
    closed();
    button?.focus();
  }
}
const serverButton = ref(null);
const menuKeys = (e) => keysIn(e, menuItems.value, () => (serverMenu.value = false), serverButton.value);

// The play modes' menu, as the server menu: opened on the player's mode.
const modeItems = ref([]);

// Choosing your match (the server's "choose-match"): a new public match, or one picked from the
// open ones (MatchPicker).
const chooseOffered = ref(false);
const picking = ref(false);
onMounted(async () => (chooseOffered.value = await call('choose_match_offered')));
function newMatch(mode) {
  menuOpen.value = false;
  call('new_match', { mode });
}
function cancelNewMatch() {
  menuOpen.value = false;
  call('cancel_new_match');
}
const modesButton = ref(null);
function toggleModes() {
  serverMenu.value = false;
  menuOpen.value = !menuOpen.value;
  if (menuOpen.value) nextTick(() => modeItems.value[Math.max(0, MODES.indexOf(props.info.play_mode))]?.focus());
}
const modeKeys = (e) => keysIn(e, modeItems.value, () => (menuOpen.value = false), modesButton.value);

async function switchTo(row) {
  serverMenu.value = false;
  // A community server: switched to at once, in the activity bar.
  if (!row.current) await call('switch_server', { host: row.host }).catch(() => {});
}

function pingClass(ms) {
  if (ms == null) return 'none';
  return ms <= 90 ? 'good' : ms <= 180 ? '' : 'far';
}

// The Squad tab: friends (fetched on the Rust side every half minute), others online, what's
// played on the server.
const squad = ref(null);
const blockMenu = ref(null);
const confirmSwitch = ref(null);

async function loadSquad(fresh) {
  squad.value = await call('squad', { fresh });
}

async function joinFriend(f) {
  await call('join_friend', { id: f.id, name: f.name });
  emit('played');
}

async function act(action, p) {
  await call('player_action', { action, id: p.id, name: p.name });
  loadSquad(true);
}

/** A friend on another server: set up there (the setup's name step if there's no account). */
async function switchToFriend() {
  const host = confirmSwitch.value.host;
  confirmSwitch.value = null;
  await call('switch_to_friend', { host });
}

// The banners over the stage: a name the network refuses, and a Uplay emulator.
const emulatorClosed = ref(false);
async function closeEmulator() {
  emulatorClosed.value = true;
  await call('dismiss_emulator_notice');
}
async function askReview() {
  await call('ask_name_review', { name: props.info.name_not_allowed });
  emit('go', 'support');
}

// The game's art behind the stage, and whether the account is signed in (the presence dot).
const art = ref(null);
const online = computed(() => list.value?.checks.some((c) => c.id === 'account' && c.status === 'ok') ?? false);

// The Intel tab: the server's news, turning every 8 seconds.
const news = ref([]);
const newsAt = ref(0);
let turned = Date.now();

async function loadNews() {
  news.value = await call('news');
  newsAt.value = 0;
}

function turn(step) {
  if (!news.value.length) return;
  newsAt.value = (newsAt.value + step + news.value.length) % news.value.length;
  turned = Date.now();
}

// The checklist (the shared library's), gathered on the Rust side; again every half minute.
const list = ref(null);
const allChecks = ref(false);
let checksTimer = null;
let stopChecks = null;

async function loadChecks(fresh) {
  list.value = await call('checks', { fresh, all: allChecks.value });
}

async function runFix(fix) {
  if (fix === 'update-launcher') return call('update_asked');
  // Choosing a server or setting up the account is the setup's server step.
  if (!(await call('fix', { fix }))) emit('change-server');
}

async function identify() {
  await call('identify');
}

async function fixEverything() {
  await call('fix_everything');
}

// The Play button says whether the player can play and what pressing it does (the 0.4.8
// plan's P2): its label, a second line with the status, and a tone (ok, warn, fail, checking,
// muted) that the text says too, never the colour alone. With something to fix, the button is
// the fix; playing anyway stays in the mode menu.
const button = computed(() => {
  const mode = LABELS[props.info.play_mode];
  const go = () => play(props.info.play_mode);
  const failing = list.value?.checks.filter((c) => c.status === 'fail') ?? [];
  if (props.info.running) return { label: 'In game', sub: null, tone: 'ok', disabled: true, act: go };
  if (!props.info.game_dir) {
    const find = failing.find((c) => c.fix);
    return { label: 'Find the game', sub: 'Game not found', tone: 'muted', disabled: !find, act: () => find && runFix(find.fix[0]) };
  }
  if (!props.info.server) return { label: 'Choose a server', sub: 'No server yet', tone: 'muted', disabled: false, act: () => emit('change-server') };
  // A release going out or maintenance says what it means for playing now.
  if (notice.value?.blocks_play) return { label: 'Server updating', sub: notice.value.status, tone: 'muted', disabled: true, act: go };
  const newMatch = props.info.new_match ? 'New match · ' : '';
  if (!list.value) return { label: mode, sub: `${newMatch}Checking…`, tone: 'checking', disabled: false, act: go };
  if (list.value.tone === 'fail') {
    const fixes = failing.filter((c) => c.fix);
    const sub = list.value.line;
    if (list.value.busy) return { label: 'Fixing…', sub, tone: 'fail', disabled: true, act: go };
    if (fixes.length === 1) return { label: fixes[0].fix[1], sub, tone: 'fail', disabled: false, act: () => runFix(fixes[0].fix[0]) };
    if (fixes.length > 1 && list.value.fixable) return { label: `Fix ${fixes.length} things`, sub, tone: 'fail', disabled: false, act: fixEverything };
    return { label: 'See what to fix', sub, tone: 'fail', disabled: false, act: () => (tab.value = 'status') };
  }
  if (list.value.tone === 'warn' || notice.value) {
    return { label: mode, sub: newMatch + (notice.value?.status ?? list.value.line), tone: 'warn', disabled: false, act: go };
  }
  // The server and its ping are on the card beside it.
  return { label: mode, sub: props.info.new_match ? 'A new match' : list.value.line || 'Ready to play', tone: 'ok', disabled: false, act: go };
});

// The notice under the launch bar: a release going out, or maintenance (asked again as often
// as it says).
const notice = ref(null);
let noticeTimer = null;
async function loadNotice() {
  notice.value = await call('notice');
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(loadNotice, (notice.value?.every ?? 60) * 1000);
}

let squadTimer = null;
let newsTimer = null;
onMounted(async () => {
  loadChecks(false);
  loadServers(false);
  loadSquad(false);
  loadNews();
  loadNotice();
  call('key_art').then((a) => (art.value = a));
  checksTimer = setInterval(() => loadChecks(false), 30000);
  squadTimer = setInterval(() => loadSquad(false), 15000);
  newsTimer = setInterval(() => Date.now() - turned >= 8000 && turn(1), 1000);
  stopChecks = await listen('checks', () => {
    loadChecks(false);
    loadServers(false);
    loadSquad(false);
    loadNews();
  });
});
onBeforeUnmount(() => {
  clearInterval(checksTimer);
  clearInterval(squadTimer);
  clearInterval(newsTimer);
  clearTimeout(noticeTimer);
  stopChecks?.();
});
</script>

<style scoped>
.play-screen { display: flex; height: 100%; }

/* The stage: night vision, the name and the controls. */
.stage { position: relative; flex: 1; min-width: 0; padding: 26px 36px; display: flex; flex-direction: column; justify-content: space-between; overflow: hidden;
  background: radial-gradient(60% 70% at 70% 35%, rgba(155, 224, 90, .10), transparent 70%), var(--bg); }
.art { position: absolute; inset: 0; background: center / cover no-repeat; opacity: .55; pointer-events: none; }
.art::after { content: ""; position: absolute; inset: 0; background: linear-gradient(180deg, rgba(10, 14, 15, .55), rgba(10, 14, 15, .35) 40%, rgba(10, 14, 15, .92)), linear-gradient(90deg, rgba(10, 14, 15, .85), transparent 70%); }
.art-enter-active { transition: opacity 1.2s ease; }
.art-enter-from { opacity: 0; }
.glow { position: absolute; inset: -20%; background: radial-gradient(35% 35% at 70% 40%, rgba(155, 224, 90, .10), transparent 70%); animation: drift 14s ease-in-out infinite alternate; pointer-events: none; }
.scan { position: absolute; inset: 0; background: repeating-linear-gradient(0deg, rgba(155, 224, 90, .035) 0 1px, transparent 1px 4px); animation: scan 8s linear infinite; pointer-events: none; }
.hud { position: relative; display: flex; justify-content: space-between; align-items: center; min-height: 58px; }
.player { display: flex; align-items: center; gap: 12px; padding: 8px 18px 8px 8px; text-align: left; border-radius: 6px; background: rgba(12, 17, 18, .7); border: 1px solid var(--line); backdrop-filter: blur(8px); transition: border-color .2s; }
.player:hover { border-color: var(--control-line); }
.player b { display: block; font-weight: 600; }
.player small { font: 11px PlexMono, monospace; color: var(--muted); }
.avatar { position: relative; width: 40px; height: 40px; border-radius: 50%; display: grid; place-items: center; color: var(--accent); font-weight: 600; border: 1.5px solid rgba(155, 224, 90, .5); background: var(--selected); }

.hero { position: relative; }
.hero .kicker { margin: 0; font-size: 12px; letter-spacing: .2em; }
h1 { margin: 4px 0 14px; font: 700 clamp(46px, 7.4vw, 92px)/.92 PlexCond, sans-serif; letter-spacing: .01em; text-transform: uppercase; }
h1 span { color: var(--accent); text-shadow: 0 0 40px rgba(155, 224, 90, .25); }
.tag { margin: 0 0 22px; color: var(--soft); font-size: 16px; }
.reveal { opacity: 0; transform: translateY(14px); animation: reveal .7s var(--ease) forwards; animation-delay: var(--d); }

.launch { display: flex; align-items: center; gap: 20px; max-width: 940px; }
/* In a narrow window the server shrinks first, then the button's padding, so Play and its menu
   always fit. */
.server-wrap { position: relative; flex: 1 1 300px; max-width: 420px; min-width: 190px; display: flex; }
.server { flex: 1; display: flex; align-items: center; gap: 10px; padding: 10px 12px 10px 16px; text-align: left; border-radius: 4px; background: rgba(12, 17, 18, .8); border: 1px solid var(--control-line); transition: border-color .2s; backdrop-filter: blur(8px); }
.server:hover, .server.open { border-color: var(--accent); }
.server .ping { display: flex; flex-direction: column; align-items: flex-end; font: 13px PlexMono, monospace; }
.server .ping small, .server-menu .ping small { font: 12px Plex, sans-serif; color: var(--muted); }
.good { color: var(--accent); } .far { color: var(--warn); } .none { color: var(--muted); font-family: Plex, sans-serif; }
.chev { transition: transform .25s; }
.server.open .chev { transform: rotate(180deg); }
.server-menu { position: absolute; left: 0; bottom: calc(100% + 8px); width: 440px; padding: 8px; display: flex; flex-direction: column; gap: 2px; border-radius: 6px; background: rgba(12, 17, 18, .95); border: 1px solid var(--control-line); backdrop-filter: blur(14px); box-shadow: 0 18px 50px rgba(0, 0, 0, .55); z-index: 6; }
.menu-head { display: flex; align-items: center; justify-content: space-between; margin: 6px 8px 8px; font-size: 13px; }
.server-menu > .muted { margin: 4px 8px 8px; }
.server-menu > button[role="menuitemradio"] { display: flex; align-items: center; gap: 10px; padding: 10px 12px; border-radius: 4px; text-align: left; transition: background .15s; }
.server-menu > button[role="menuitemradio"]:hover, .server-menu > button[role="menuitemradio"]:focus-visible { background: var(--control); outline: none; }
.server-menu > button.on { background: var(--selected); box-shadow: inset 3px 0 0 var(--accent); }
.row-main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.row-main b { font-weight: 600; }
.row-main small { font-size: 12.5px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.server-menu .ping { display: flex; flex-direction: column; align-items: flex-end; font: 13px PlexMono, monospace; min-width: 72px; }
.tags { display: flex; gap: 6px; }
.mtag { padding: 0 6px; border-radius: 3px; font: 10px/17px PlexMono, monospace; letter-spacing: .08em; text-transform: uppercase; border: 1px solid var(--control-line); color: var(--muted); }
.mtag.best { background: var(--accent); border-color: var(--accent); color: var(--on-accent); }
.mtag.current { color: var(--accent); border-color: rgba(155, 224, 90, .4); }
.mtag.warn { color: var(--warn); border-color: rgba(240, 180, 76, .4); }
.note { font-size: 12.5px; margin: 6px 8px; }
.to-servers { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin-top: 6px; padding: 12px; border-top: 1px solid var(--line); text-align: left; font-size: 13px; }
.to-servers b { color: var(--accent); font-weight: 600; white-space: nowrap; }
.to-servers:hover b { text-decoration: underline; }
.to-friends { color: var(--accent); text-decoration: underline; }
.server > span:first-child { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.server b { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.chev { width: 16px; height: 16px; color: var(--muted); }
.play { position: relative; height: 58px; padding: 0 clamp(26px, 4.5vw, 64px); background: var(--accent); color: var(--on-accent); font: 700 24px PlexCond, sans-serif; letter-spacing: .14em; text-transform: uppercase; overflow: hidden;
  clip-path: polygon(0 0, 100% 0, 100% 75%, calc(100% - 14px) 100%, 0 100%); transition: transform .12s, filter .2s; }
.play::after { content: ""; position: absolute; top: 0; bottom: 0; width: 40%; left: -60%; background: linear-gradient(100deg, transparent, rgba(255, 255, 255, .35), transparent); animation: sheen 4.5s ease-in-out infinite; }
.play:hover:not(:disabled) { filter: brightness(1.08); }
.play:active:not(:disabled) { transform: scale(.98); }
.play:disabled, .play-more:disabled { cursor: not-allowed; filter: saturate(.6) brightness(.85); }
.split { position: relative; display: flex; flex: none; margin-left: auto; }
/* The label, and under it the status (P2). */
.play { min-width: 210px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px; }
/* Tight around the capitals (line-height under 1), and the letter spacing after the last letter
   given back on the left, so the word sits in the middle over the status. */
.play .label { line-height: .78; white-space: nowrap; padding-left: .14em; }
.split.fail .play .label, .split.muted .play .label { padding-left: .08em; }
/* The status never widens the button (the label does): it takes the width there is. */
/* margin: 0, as the page's own .sub (a heading's subtitle) would push it 14px down. */
.play .sub { margin: 0; width: 0; min-width: 100%; text-align: center; font: 600 11.5px/1 Plex, sans-serif; letter-spacing: .02em; text-transform: none; opacity: .82; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
/* Its tone: ready (the accent), warnings, something to fix, can't play now, checking. */
.split.warn .play, .split.warn .play-more { background: var(--warn); }
.split.fail .play, .split.fail .play-more { background: var(--bad); }
.split.fail .play, .split.muted .play { padding: 0 clamp(18px, 2.4vw, 40px); }
.split.fail .play { font-size: 20px; letter-spacing: .08em; }
.split.warn .play, .split.fail .play { animation: none; }
.split.muted .play, .split.muted .play-more { background: var(--control); color: var(--fg); border-color: var(--control-line); }
.split.muted .play { font-size: 20px; letter-spacing: .08em; animation: none; }
.split.muted .play::after, .split.fail .play::after, .split.warn .play::after { display: none; }
.split.muted .play .sub { color: var(--soft); opacity: 1; }
.split.checking .play { animation: breathe 1.6s ease-in-out infinite; }
.play { clip-path: none; border-radius: 4px 0 0 4px; animation: pulse 2.8s ease-in-out infinite; }
.play:disabled { animation: none; }
.play-more { height: 58px; width: 42px; display: grid; place-items: center; background: var(--accent); color: var(--on-accent); border-left: 2px solid var(--bg); clip-path: polygon(0 0, 100% 0, 100% 75%, 75% 100%, 0 100%); }
.play-more svg { width: 18px; height: 18px; stroke-width: 2.4; }
.modes { position: absolute; right: 0; bottom: calc(100% + 10px); width: 300px; max-height: calc(100vh - 200px); overflow-y: auto; padding: 6px; display: flex; flex-direction: column; border-radius: 6px; background: rgba(12, 17, 18, .94); border: 1px solid var(--control-line); backdrop-filter: blur(14px); box-shadow: 0 18px 50px rgba(0, 0, 0, .55); z-index: 5; }
.modes button { display: flex; flex-direction: column; align-items: flex-start; padding: 10px 12px; border-radius: 4px; text-align: left; transition: background .15s; }
.modes button:hover { background: var(--control); }
.modes .sep { height: 1px; margin: 4px 6px; background: var(--line); }
.modes button.on { background: var(--selected); box-shadow: inset 3px 0 0 var(--accent); }
.modes b { font-weight: 600; }
.modes small { color: var(--muted); font-size: 12.5px; }
.menu-enter-active, .menu-leave-active { transition: opacity .18s, transform .18s; }
.menu-enter-from, .menu-leave-to { opacity: 0; transform: translateY(8px) scale(.98); }

/* The drawer: tabs with a sliding underline. */
.drawer { width: clamp(300px, 30vw, 380px); flex: none; display: flex; flex-direction: column; background: var(--surface); border-left: 1px solid var(--line); animation: slide .5s var(--ease); }
.tabs { position: relative; display: flex; border-bottom: 1px solid var(--line); }
.tabs button { flex: 1; height: 52px; font: 11.5px PlexMono, monospace; letter-spacing: .14em; text-transform: uppercase; color: var(--muted); transition: color .2s; }
.tabs button:hover { color: var(--soft); }
.tabs button.on { color: var(--fg); }
.underline { position: absolute; left: 0; bottom: -1px; height: 2px; background: var(--accent); box-shadow: 0 0 10px rgba(155, 224, 90, .6); transition: transform .3s var(--ease); }
.pane { flex: 1; padding: 16px 18px; }
.pane ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
.pane li { display: flex; align-items: center; gap: 12px; padding: 10px; border-radius: 6px; transition: background .2s; }
.pane li:hover { background: var(--control); }
.pane li .edge { width: 3px; align-self: stretch; border-radius: 2px; background: var(--accent); }
.pane li .edge.busy { background: var(--warn); }
.pane li > span:nth-child(2) { flex: 1; min-width: 0; }
.pane li b { display: block; font-weight: 600; }
.pane li small { display: block; font: 11px PlexMono, monospace; color: var(--muted); overflow-wrap: anywhere; }

.status { display: flex; flex-direction: column; }
.check { align-items: flex-start !important; }
.pane li.check small { margin-top: 2px; font: 12.5px/1.4 Plex, sans-serif; color: var(--muted); }
.mark { width: 9px; height: 9px; margin-top: 6px; border-radius: 50%; flex: none; background: var(--accent); }
.mark.warn { background: var(--warn); }
.mark.fail { background: var(--bad); }
.fixbtn { display: inline-block; margin-top: 6px; height: 30px; padding: 0 14px; border-radius: 4px; border: 1px solid rgba(155, 224, 90, .35); color: var(--accent); font-weight: 600; font-size: 13.5px; transition: background .2s, border-color .2s; }
.fixbtn:hover:not(:disabled) { background: var(--selected); border-color: var(--accent); }
.fixbtn:disabled { opacity: .45; cursor: default; }
.foot { margin-top: auto; padding-top: 14px; display: flex; flex-direction: column; gap: 10px; }
.links { margin: 0; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13.5px; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
.squad, .intel { display: flex; flex-direction: column; }
.squad li > button { flex: none; }
.pane.squad li > span:nth-child(2) small { font: 12px/1.4 Plex, sans-serif; letter-spacing: 0; text-transform: none; }
.sub { margin: 14px 0 4px; font-size: 13px; }
.squad .foot { gap: 4px; }
.comms { margin-top: 14px; padding: 12px; border-radius: 4px; background: var(--rail); border: 1px solid var(--line); display: flex; flex-direction: column; gap: 6px; }
.comms p { margin: 0; }
.comms .btn { width: 100%; height: 34px; }
.squad .foot p { margin: 0; }
.small { font-size: 12.5px; }
.accent { color: var(--accent); }
.line { margin: 2px 0; font-size: 13px; }
.block { margin-top: 8px; padding: 10px 12px; border-radius: 4px; background: var(--sunken); border: 1px solid rgba(255, 122, 107, .4); display: flex; flex-direction: column; gap: 8px; font-size: 13px; }
.row { display: flex; gap: 8px; }
.row.end { justify-content: flex-end; margin-top: 16px; }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.btn.danger { border-color: var(--bad); color: var(--bad); }
.intel h3 { margin: 6px 0 4px; font-size: 16.5px; font-weight: 600; }
.intel article p { margin: 0; font-size: 14px; }
.dots { flex-direction: row !important; align-items: center; gap: 4px !important; }
.dots button { width: 22px; height: 22px; display: grid; place-items: center; color: var(--muted); }
.dots button svg { width: 14px; height: 14px; }
.dots button:hover { color: var(--fg); }
.dot::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--control-line); transition: background .2s, transform .2s; }
.dot.on::before { background: var(--accent); transform: scale(1.35); }
.grow { flex: 1; }
.backdrop { position: fixed; inset: 0; z-index: 40; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(540px, 100%); padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.dialog h2 { margin: 2px 0 10px; font: 700 26px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
.mono { font: 13px PlexMono, monospace; font-weight: 400; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
.notice { margin-top: 18px; max-width: 940px; padding: 14px 20px 16px; border-radius: 6px; background: rgba(240, 180, 76, .07); border: 1px solid rgba(240, 180, 76, .35); }
.notice p { margin: 0; }
.notice-top { display: flex; gap: 16px; justify-content: space-between; align-items: flex-start; }
.notice-head { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; margin-bottom: 4px !important; }
.notice-head b { font: 700 16px PlexCond, sans-serif; text-transform: uppercase; letter-spacing: .02em; color: var(--warn); }
.eta { padding: 1px 8px; border-radius: 4px; background: rgba(240, 180, 76, .16); font: 13px PlexMono, monospace; }
.notice .soft { font-size: 14px; }
.notice-act { display: flex; flex-direction: column; align-items: flex-end; gap: 6px; flex: none; }
.notice-servers { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 8px; margin-top: 10px; }
.nbox { padding: 8px 12px; border-radius: 6px; background: rgba(10, 14, 15, .55); border: 1px solid var(--line); }
.nbox.mine { border-color: rgba(240, 180, 76, .55); }
.nbox p:first-child { display: flex; justify-content: space-between; gap: 8px; font-size: 13.5px; }
.nbox .mono { font-size: 12px; }
.doing { font-size: 12.5px; color: var(--muted); }
.doing.now { color: var(--warn); } .doing.done { color: var(--accent); }
.meter { display: block; height: 3px; margin-top: 6px; border-radius: 2px; background: var(--line); overflow: hidden; }
.meter i { display: block; height: 100%; background: var(--warn); transition: width .4s var(--ease); }
.avatar i { position: absolute; right: -2px; bottom: -2px; width: 12px; height: 12px; border-radius: 50%; background: var(--muted); border: 3px solid var(--bg); }
.avatar i.on { background: var(--accent); }
.beta { margin: 10px 0 0; max-width: 940px; color: var(--warn); font-size: 13px; }
.newsline { display: flex; align-items: center; gap: 12px; margin-top: 22px; padding-top: 14px; border-top: 1px solid var(--control-line); color: var(--soft); max-width: 940px; }
.newsline .headline { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.fade-enter-active, .fade-leave-active { transition: opacity .4s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
.banners { position: relative; z-index: 2; display: flex; flex-direction: column; gap: 8px; margin: -6px 0 12px; }
.banner { display: flex; align-items: center; gap: 12px; padding: 10px 14px; border-radius: 6px; font-size: 14px; }
.banner span { flex: 1; }
.banner.bad { background: rgba(255, 122, 107, .12); border: 1px solid rgba(255, 122, 107, .4); }
.banner.warnbg { background: rgba(240, 180, 76, .12); border: 1px solid rgba(240, 180, 76, .35); }
.banner .x { width: 28px; height: 28px; border-radius: 4px; display: grid; place-items: center; color: var(--soft); flex: none; }
.banner .x:hover { background: var(--control-hover); color: var(--fg); }
.banner .x svg { width: 10px; height: 10px; stroke-width: 1.4; }
@keyframes reveal { to { opacity: 1; transform: none; } }
@keyframes drift { to { transform: translate(-6%, 4%) scale(1.08); } }
@keyframes scan { to { background-position: 0 64px; } }
@keyframes ping { 0% { box-shadow: 0 0 0 0 rgba(240, 180, 76, .5); } 70%, 100% { box-shadow: 0 0 0 9px rgba(240, 180, 76, 0); } }
@keyframes pulse { 50% { box-shadow: 0 0 34px rgba(155, 224, 90, .35); } }
@keyframes ping-bad { 0% { box-shadow: 0 0 0 0 rgba(255, 122, 107, .5); } 70%, 100% { box-shadow: 0 0 0 9px rgba(255, 122, 107, 0); } }
@keyframes spin { to { transform: rotate(360deg); } }
@keyframes breathe { 50% { filter: brightness(.88); } }
@keyframes sheen { 0%, 60% { left: -60%; } 100% { left: 130%; } }
@keyframes slide { from { transform: translateX(40px); opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .reveal { opacity: 1; transform: none; } }
</style>
