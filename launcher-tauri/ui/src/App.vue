<template>
  <div class="window" :class="{ framed: !maximised }">
    <TitleBar :product="info.product" :release="info.release" />
    <div class="body">
      <Rail :current="screen" :release="info.release" :community="info.community" @go="go" />
      <main class="screen">
        <Transition name="view" mode="out-in">
          <Play v-if="screen === 'play'" key="play" :info="info" @go="go" />
          <Placeholder v-else :key="screen" v-bind="PLACEHOLDERS[screen]" />
        </Transition>
      </main>
    </div>
  </div>
</template>

<script setup>
// The launcher's window: its own title bar, the side menu, and the screen chosen, crossing over
// as it changes. The screens not moved over from the egui launcher yet say so (step 2).
import { onMounted, reactive, ref } from 'vue';
import TitleBar from './components/TitleBar.vue';
import Rail from './components/Rail.vue';
import Play from './views/Play.vue';
import Placeholder from './views/Placeholder.vue';
import { call, inTauri } from './lib/tauri.js';

const PLACEHOLDERS = {
  servers: { kicker: 'Servers', title: 'Servers', text: 'The servers you can play on, how far they are, and your accounts on them.' },
  chat: { kicker: 'Comms', title: 'Chat', text: 'Global, Friends and your lobby, across the community network.' },
  intel: { kicker: 'Intel', title: 'News and the roadmap', text: "What's new, and what's coming." },
  support: { kicker: 'Support', title: 'Support', text: 'Ask for help, send your logs, and read the manual.' },
  settings: { kicker: 'Settings', title: 'Settings', text: 'The game, the launcher, beta features and the advanced options.' },
};

const info = reactive({ product: '5th Echelon Enhanced', release: '', game_dir: null, server: null, account: null, community: true });
const screen = ref('play');
const maximised = ref(false);

function go(id) {
  screen.value = id;
}

onMounted(async () => {
  Object.assign(info, await call('info'));
  // No edge round a maximised window.
  if (inTauri) {
    const w = window.__TAURI__.window.getCurrentWindow();
    const check = async () => (maximised.value = await w.isMaximized());
    check();
    w.onResized(check);
  }
});
</script>

<style scoped>
.window { height: 100%; display: flex; flex-direction: column; }
.window.framed { border: 1px solid var(--control-line); }
.body { flex: 1; min-height: 0; display: flex; }
.screen { flex: 1; min-width: 0; position: relative; }
.screen > * { height: 100%; }
</style>
