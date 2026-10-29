<template>
  <div class="window" :class="{ framed: !maximised }">
    <TitleBar :product="info.product" :release="info.release" />
    <div class="body">
      <Rail :current="screen" :release="info.release" :community="info.community" @go="go" @version="whatsNew?.again()" />
      <div class="column">
        <main class="screen">
          <Transition name="view" mode="out-in">
            <Setup v-if="screen === 'play' && setupShown" key="setup" :info="info" :fresh="fresh" @changed="refresh" @cancel="editing = false" @finished="finishSetup" />
          <Play v-else-if="screen === 'play'" key="play" :info="info" @go="go" @played="played" @change-server="editing = true" />
            <Servers v-else-if="screen === 'servers'" key="servers" :info="info" @go="go" @account="(a) => (askedName = a)" @changed="refresh" />
          <Placeholder v-else :key="screen" v-bind="PLACEHOLDERS[screen]" />
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
// as it changes. The screens not moved over from the egui launcher yet say so (step 2).
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import TitleBar from './components/TitleBar.vue';
import Rail from './components/Rail.vue';
import Play from './views/Play.vue';
import Setup from './views/Setup.vue';
import Servers from './views/Servers.vue';
import Placeholder from './views/Placeholder.vue';
import ActivityBar from './components/ActivityBar.vue';
import Questions from './components/Questions.vue';
import Feedback from './components/Feedback.vue';
import AccountDialog from './components/AccountDialog.vue';
import WhatsNew from './components/WhatsNew.vue';
import { call, inTauri, listen } from './lib/tauri.js';

const PLACEHOLDERS = {
  host: { kicker: 'Host on this PC', title: 'Host a server', text: 'Run a server for your group: a LAN party, or friends over the internet.' },
  chat: { kicker: 'Comms', title: 'Chat', text: 'Global, Friends and your lobby, across the community network.' },
  intel: { kicker: 'Intel', title: 'News and the roadmap', text: "What's new, and what's coming." },
  support: { kicker: 'Support', title: 'Support', text: 'Ask for help, send your logs, and read the manual.' },
  settings: { kicker: 'Settings', title: 'Settings', text: 'The game, the launcher, beta features and the advanced options.' },
};

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

function go(id) {
  screen.value = id;
}

async function refresh() {
  Object.assign(info, await call('info'));
  loaded.value = true;
}

/** After Play: what it said is in the activity bar; a question it waits on comes up. */
async function played() {
  await Promise.all([refresh(), bar.value?.refresh(), questions.value?.refresh()]);
}

onMounted(async () => {
  await refresh();
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
