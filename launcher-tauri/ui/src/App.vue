<template>
  <div class="window" :class="{ framed: !maximised }">
    <TitleBar :product="info.product" :release="info.release" />
    <UpdateBanner />
    <div class="body">
      <Rail :current="screen === 'host' ? 'servers' : screen" :release="info.release" :community="info.community" :unread="glance.unread" @go="go" @version="whatsNew?.again()" />
      <div class="column">
        <main class="screen">
          <Transition name="view" mode="out-in">
            <Setup v-if="screen === 'play' && setupShown" key="setup" :info="info" :fresh="fresh" @changed="refresh" @cancel="editing = false" @finished="finishSetup" />
          <Play v-else-if="screen === 'play'" key="play" :info="info" :latest="glance.latest" @go="go" @played="played" @change-server="editing = true" />
            <Servers v-else-if="screen === 'servers'" key="servers" :info="info" @go="go" @account="(a) => (askedName = a)" @changed="refresh" />
          <Intel v-else-if="screen === 'intel'" key="intel" :info="info" />
          <Support v-else-if="screen === 'support'" key="support" @go="go" />
          <Chat v-else-if="screen === 'chat'" key="chat" />
          <Settings v-else-if="screen === 'settings'" key="settings" :open="settingsAt" :size="size" @size="setSize" @fullscreen="toggleFullscreen" @changed="refresh" />
          <Host v-else-if="screen === 'host'" key="host" />
          </Transition>
        </main>
        <ActivityBar ref="bar" />
      </div>
    </div>
    <AccountDialog :asked="askedName" :max-name="info.max_name" @close="askedName = null" />
    <Feedback />
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
  size.value = Math.min(1.6, Math.max(0.7, Math.round(to * 10) / 10));
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

onMounted(async () => {
  await refresh();
  document.addEventListener('keydown', keys);
  // The size kept from before.
  const kept = (await call('settings')).size;
  if (kept && Math.abs(kept - 1) > 0.01) setSize(kept);
  // Whether the game is running, and settings changed elsewhere (the egui launcher, by hand).
  timer = setInterval(refresh, 3000);
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
  document.removeEventListener('keydown', keys);
  clearInterval(timer);
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
</style>
