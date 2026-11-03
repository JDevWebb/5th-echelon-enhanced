<template>
  <div class="window" :class="{ framed: !maximised }">
    <TitleBar :product="info.product" :release="info.release" />
    <UpdateBanner />
    <div class="body">
      <Rail :current="screen === 'host' ? 'servers' : screen" :release="info.release" :community="info.community" :unread="glance.unread" :requests="info.requests || 0" @go="go" @version="whatsNew?.again()" />
      <div class="column">
        <main class="screen">
          <Transition name="view" mode="out-in">
            <Setup v-if="screen === 'play' && setupShown" key="setup" :info="info" :fresh="fresh" @changed="refresh" @cancel="editing = false" @finished="finishSetup" />
          <Play v-else-if="screen === 'play'" key="play" :info="info" :latest="glance.latest" @go="go" @played="played" @change-server="editing = true" />
            <Matches v-else-if="screen === 'matches'" key="matches" :info="info" :watching="watching" @go="go" @watch="setWatch" @played="played" />
            <Servers v-else-if="screen === 'servers'" key="servers" :info="info" @go="go" @account="(a) => (askedName = a)" @changed="refresh" />
          <Intel v-else-if="screen === 'intel'" key="intel" :info="info" />
          <Support v-else-if="screen === 'support'" key="support" @go="go" />
          <Chat v-else-if="screen === 'chat'" key="chat" />
          <Friends v-else-if="screen === 'friends'" key="friends" :info="info" @go="go" @changed="refresh" />
          <Settings v-else-if="screen === 'settings'" key="settings" :open="settingsAt" :size="size" @size="setSize" @fullscreen="toggleFullscreen" @changed="refresh" />
          <Host v-else-if="screen === 'host'" key="host" />
          </Transition>
        </main>
        <Transition name="modal">
          <div v-if="seatOpen" class="seat" role="status">
            <p><b>A seat opened</b> in {{ seatOpen.label }}.</p>
            <button type="button" class="btn primary" @click="joinWatched">Join</button>
            <button type="button" class="btn" @click="seatOpen = null">Not now</button>
          </div>
        </Transition>
        <ActivityBar ref="bar" />
      </div>
    </div>
    <AccountDialog :asked="askedName" :max-name="info.max_name" @close="askedName = null" />
    <Feedback />
    <Transition name="modal">
      <div v-if="closing" class="backdrop">
        <div v-modal class="dialog" role="dialog" aria-modal="true" aria-labelledby="closing-title">
          <h2 id="closing-title">Your report is still sending</h2>
          <p class="soft">Wait and the launcher closes as soon as it's sent, or close now and it's sent the next time you start the launcher.</p>
          <p v-if="closing === 'waiting'" class="muted working"><i class="spin"></i>Waiting for it to send…</p>
          <div class="row end">
            <button type="button" class="btn" @click="closeNow">Close now, send it next time</button>
            <button type="button" class="btn primary" :disabled="closing === 'waiting'" @click="waitThenClose">Wait, then close</button>
          </div>
        </div>
      </div>
    </Transition>
    <WhatsNew ref="whatsNew" :waiting="questionsUp" :community="info.community" @network="refresh" />
    <Questions ref="questions" :with-options="!(setupShown && fresh)" @up="(up) => (questionsUp = up)" />
  </div>
</template>

<script setup>
// The launcher's window: its own title bar, the side menu, and the screen chosen, crossing over
// as it changes.
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import TitleBar from './components/TitleBar.vue';
import Rail from './components/Rail.vue';
import Play from './views/Play.vue';
import Setup from './views/Setup.vue';
import Servers from './views/Servers.vue';
import Intel from './views/Intel.vue';
import Support from './views/Support.vue';
import Chat from './views/Chat.vue';
import Friends from './views/Friends.vue';
import Matches from './views/Matches.vue';
import Settings from './views/Settings.vue';
import Host from './views/Host.vue';
import ActivityBar from './components/ActivityBar.vue';
import UpdateBanner from './components/UpdateBanner.vue';
import Questions from './components/Questions.vue';
import Feedback from './components/Feedback.vue';
import AccountDialog from './components/AccountDialog.vue';
import WhatsNew from './components/WhatsNew.vue';
import { call, inTauri, listen } from './lib/tauri.js';

const info = reactive({ product: '5th Echelon Enhanced', release: '', game_dir: null, server: null, account: null, community: true, play_mode: 'play', managed: false, running: false });
const screen = ref('play');
// The guided setup: until there's a game and a server (or while changing server), and on a
// first setup until its game options are chosen. Never where another tool manages the game.
const editing = ref(false);
const fresh = ref(false);
const loaded = ref(false);
const setupShown = computed(() => loaded.value && !info.managed && (!info.game_dir || !info.server || editing.value || fresh.value));
watch(
  () => loaded.value && !info.managed && (!info.game_dir || !info.server),
  (first) => first && (fresh.value = true),
);

// A server with no account yet, from outside the setup (a friend's): the account dialog.
const askedName = ref(null);
let stopSetup = null;

function finishSetup() {
  askedName.value = null;
  editing.value = false;
  fresh.value = false;
  refresh();
}
const maximised = ref(false);

const bar = ref(null);
const whatsNew = ref(null);
// The first-run questions are up: What's new waits for them.
const questionsUp = ref(true);
const questions = ref(null);
let timer = null;
let fastTimer = null;

// "Tell me when a seat opens" (the Matches tab): the match watched, asked about every 10 s
// wherever the player is in the launcher; once it's joinable, a line with Join, and the window
// asks for attention. A match that's gone ends the watch.
const watching = ref(null);
const seatOpen = ref(null);
let watchTimer = null;
function setWatch(w) {
  watching.value = w;
  seatOpen.value = null;
  clearInterval(watchTimer);
  if (w) watchTimer = setInterval(checkWatched, 10000);
}
async function checkWatched() {
  const w = watching.value;
  if (!w) return;
  const got = await call('lobbies');
  const l = got?.lobbies?.find((x) => x.room === w.room);
  if (!l) return setWatch(null);
  if (l.joinable) {
    setWatch(null);
    seatOpen.value = w;
    if (inTauri) window.__TAURI__.window.getCurrentWindow().requestUserAttention(2);
  }
}
async function joinWatched() {
  const w = seatOpen.value;
  seatOpen.value = null;
  if (w) {
    await call('join_match', { room: w.room });
    played();
  }
}

// The window always has room for the full layout (the Matches tab's list beside its details):
// at least FULL at the player's size; a screen too small for that at their size gets a smaller
// size instead (never below 0.7), so the pages are never squeezed into a narrow form.
const FULL = { width: 1280, height: 800 };
async function fitWindow(want) {
  if (!inTauri) return want;
  const w = window.__TAURI__.window.getCurrentWindow();
  const { LogicalSize } = window.__TAURI__.dpi;
  const monitor = await window.__TAURI__.window.currentMonitor();
  let fits = want;
  if (monitor) {
    const scale = monitor.scaleFactor || 1;
    const area = monitor.workArea?.size || monitor.size;
    const room = { width: area.width / scale, height: area.height / scale };
    fits = Math.min(want, room.width / FULL.width, room.height / FULL.height);
    fits = Math.max(0.7, Math.floor(fits * 10) / 10);
  }
  const min = new LogicalSize(Math.round(FULL.width * fits), Math.round(FULL.height * fits));
  await w.setMinSize(min);
  const now = await w.innerSize();
  const scale = await w.scaleFactor();
  if (now.width / scale < min.width || now.height / scale < min.height) {
    await w.setSize(new LogicalSize(Math.max(now.width / scale, min.width), Math.max(now.height / scale, min.height)));
  }
  return fits;
}

/** To a screen; for Settings, at a section ("network", "identity"…). */
function go(id, at = null) {
  screen.value = id;
  if (id === 'settings') settingsAt.value = at;
}
const settingsAt = ref(null);

// The player's size (Settings › Display, Ctrl + / Ctrl − / Ctrl 0) as the page's zoom, and
// full screen (F11).
const size = ref(1);
async function setSize(to) {
  const want = Math.min(1.6, Math.max(0.7, Math.round(to * 10) / 10));
  // As large as asked while the full layout still fits the screen.
  size.value = await fitWindow(want);
  await call('set', { key: 'size', value: size.value });
  if (inTauri) window.__TAURI__.webview.getCurrentWebview().setZoom(size.value);
}
async function toggleFullscreen() {
  if (!inTauri) return;
  const w = window.__TAURI__.window.getCurrentWindow();
  w.setFullscreen(!(await w.isFullscreen()));
}
function keys(e) {
  if (e.key === 'F11') {
    e.preventDefault();
    toggleFullscreen();
  } else if (e.ctrlKey && (e.key === '+' || e.key === '=')) {
    e.preventDefault();
    setSize(size.value + 0.1);
  } else if (e.ctrlKey && e.key === '-') {
    e.preventDefault();
    setSize(size.value - 0.1);
  } else if (e.ctrlKey && e.key === '0') {
    e.preventDefault();
    setSize(1);
  }
}

async function refresh() {
  Object.assign(info, await call('info'));
  loaded.value = true;
  if (info.community) Object.assign(glance, await call('chat_glance'));
}

// Chat at a glance: unseen mentions and messages (the side menu's dot), the latest in Global.
const glance = reactive({ unread: 0, latest: null });

/** After Play: what it said is in the activity bar; a question it waits on comes up. */
async function played() {
  await Promise.all([refresh(), bar.value?.refresh(), questions.value?.refresh()]);
}

// Closing while a report sends: wait for it, or close and send it next time (it's kept either
// way), as egui asks.
const closing = ref(null);
let closeTimer = null;
function closeNow() {
  window.__TAURI__?.window.getCurrentWindow().destroy();
}
function waitThenClose() {
  closing.value = 'waiting';
  closeTimer = setInterval(async () => {
    if (!(await call('report_sending'))) closeNow();
  }, 500);
}

onMounted(async () => {
  await refresh();
  // A server that moved its players (global matchmaking): the launcher follows, once.
  call('follow_move').then((moved) => moved && refresh());
  if (inTauri) {
    window.__TAURI__.window.getCurrentWindow().onCloseRequested(async (event) => {
      if (closing.value || !(await call('report_sending'))) return;
      event.preventDefault();
      closing.value = 'asking';
    });
  }
  document.addEventListener('keydown', keys);
  // The size kept from before.
  const kept = (await call('settings')).size;
  // The window sized for the full layout at the player's size (or what the screen allows).
  setSize(kept || 1);
  // Whether the game is running, and settings changed elsewhere (the egui launcher, by hand).
  timer = setInterval(refresh, 3000);
  // While the game is being got ready (Play waiting), sooner: Play turns on within half a second.
  fastTimer = setInterval(() => info.preparing && refresh(), 500);
  stopSetup = await listen('setup', ({ payload }) => {
    // Outside the guided setup (a friend's server, the menu's): the account dialog.
    if (payload.kind === 'needs-name' && !setupShown.value) askedName.value = payload;
  });
  // No edge round a maximised window.
  if (inTauri) {
    const w = window.__TAURI__.window.getCurrentWindow();
    const check = async () => (maximised.value = await w.isMaximized());
    check();
    w.onResized(check);
  }
});
onBeforeUnmount(() => {
  clearInterval(closeTimer);
  document.removeEventListener('keydown', keys);
  clearInterval(timer);
  clearInterval(fastTimer);
  clearInterval(watchTimer);
  stopSetup?.();
});
</script>

<style scoped>
.window { height: 100%; display: flex; flex-direction: column; }
.window.framed { border: 1px solid var(--control-line); }
.body { flex: 1; min-height: 0; display: flex; }
.column { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.screen { flex: 1; min-height: 0; position: relative; }
.screen > * { height: 100%; }
.backdrop { position: fixed; inset: 0; z-index: 70; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(460px, 100%); padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.dialog h2 { margin: 0 0 10px; font: 700 22px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
.dialog p { margin: 0 0 10px; }
.row { display: flex; gap: 10px; }
.row.end { justify-content: flex-end; margin-top: 8px; }
.btn:disabled { opacity: .45; cursor: default; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.seat { position: absolute; right: 24px; bottom: 76px; z-index: 40; display: flex; align-items: center; gap: 12px; padding: 12px 14px 12px 18px; border-radius: 6px; background: var(--surface); border: 1px solid rgba(155, 224, 90, .45); box-shadow: 0 18px 50px rgba(0, 0, 0, .55); }
.seat p { margin: 0; }
.column { position: relative; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
</style>
