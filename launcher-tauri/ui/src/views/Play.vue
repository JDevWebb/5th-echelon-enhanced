<template>
  <div class="play-screen">
    <section class="stage">
      <div class="scan" aria-hidden="true"></div>
      <div class="glow" aria-hidden="true"></div>
      <div class="hud">
        <span class="caps">{{ inTauri ? 'Field ops' : 'Browser preview' }}</span>
        <button v-if="info.account" class="player" type="button" @click="$emit('go', 'servers')">
          <span class="avatar">{{ info.account[0].toUpperCase() }}</span>
          <span><b>{{ info.account }}</b><small>{{ info.server }}</small></span>
        </button>
      </div>
      <div class="hero">
        <p class="kicker reveal" style="--d: 0ms">Tom Clancy's</p>
        <h1 class="reveal" style="--d: 80ms">Splinter Cell<br /><span>Blacklist</span></h1>
        <p class="tag reveal" style="--d: 180ms">Co-op and Spies vs Mercs on community servers. No VPN, no port forwarding.</p>
        <div class="launch reveal" style="--d: 260ms">
          <button class="server" type="button" title="Change server" @click="$emit('change-server')">
            <span><span class="caps">Server</span><b>{{ info.server || 'None chosen' }}</b></span>
            <svg class="chev" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6" /></svg>
          </button>
          <span class="ready"><i :class="tone"></i><span>{{ readyText }}</span></span>
          <span class="split">
            <button class="play" type="button" :disabled="!!blocked" :title="blocked || null" @click="play(info.play_mode)">
              <span>{{ info.running ? 'In game' : LABELS[info.play_mode] }}</span>
            </button>
            <button
              class="play-more"
              type="button"
              aria-label="Play SvM or Co-op"
              aria-haspopup="menu"
              :aria-expanded="menuOpen"
              :disabled="!!blocked || info.managed"
              :title="info.managed ? 'This game is set up by another tool, so the launcher starts it as it is.' : null"
              @click.stop="menuOpen = !menuOpen"
            >
              <svg viewBox="0 0 24 24"><path d="M6 15l6-6 6 6" /></svg>
            </button>
            <Transition name="menu">
              <span v-if="menuOpen" class="modes" role="menu">
                <button
                  v-for="mode in MODES"
                  :key="mode"
                  type="button"
                  role="menuitemradio"
                  :aria-checked="info.play_mode === mode"
                  :class="{ on: info.play_mode === mode }"
                  @click="choose(mode)"
                >
                  <b>{{ LABELS[mode] }}</b><small>{{ HINTS[mode] }}</small>
                </button>
              </span>
            </Transition>
          </span>
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
              <a href="#" @click.prevent="$emit('go', 'settings')">Connection test</a>
            </p>
            <button v-if="list.fixable" type="button" class="btn primary" @click="fixEverything">Fix everything</button>
          </div>
        </section>
        <section v-else :key="tab" class="pane">
          <p class="muted">{{ tab === 'squad' ? 'Friends and Global come across' : 'The news comes across' }} in the next step.</p>
        </section>
      </Transition>
    </aside>
  </div>
</template>

<script setup>
// The Play screen, the trial's (trials/tauri-play) on the launcher's real data: the game, the
// server and the account, and Play (with its modes) starting the game through the shared
// library. The server menu, friends and the news come next.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { call, inTauri, listen } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true } });
const emit = defineEmits(['go', 'played', 'change-server']);

// What the Play button starts (the shared library's PlayMode).
const MODES = ['play', 'svm', 'coop'];
const LABELS = { play: 'Play', svm: 'Play SvM', coop: 'Play Coop' };
const HINTS = { play: 'The game as it is', svm: 'Straight into a Spies vs Mercs lobby', coop: 'Straight into a co-op lobby' };

const menuOpen = ref(false);
const closeMenu = () => (menuOpen.value = false);
onMounted(() => document.addEventListener('click', closeMenu));
onBeforeUnmount(() => document.removeEventListener('click', closeMenu));

/** Why Play can't start now, if it can't. */
const blocked = computed(() => {
  if (!props.info.game_dir) return "Choose the game's folder first.";
  if (props.info.running) return 'The game is running.';
  return null;
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

const tabs = computed(() => [
  { id: 'status', label: 'Status' },
  ...(props.info.community ? [{ id: 'squad', label: 'Squad' }] : []),
  { id: 'intel', label: 'Intel' },
]);
const tab = ref('status');
const tabIndex = computed(() => Math.max(0, tabs.value.findIndex((t) => t.id === tab.value)));

// The checklist (the shared library's), gathered on the Rust side; again every half minute.
const list = ref(null);
const allChecks = ref(false);
let checksTimer = null;
let stopChecks = null;

async function loadChecks(fresh) {
  list.value = await call('checks', { fresh, all: allChecks.value });
}

async function runFix(fix) {
  // Choosing a server or setting up the account is the setup's server step.
  if (!(await call('fix', { fix }))) emit('change-server');
}

async function identify() {
  await call('identify');
}

async function fixEverything() {
  await call('fix_everything');
}

const tone = computed(() => (props.info.running ? 'ok' : list.value?.tone ?? 'none'));
const readyText = computed(() => {
  if (!props.info.game_dir) return 'Game not found';
  if (!props.info.server) return 'Choose a server';
  return list.value?.line ?? 'Checking…';
});

onMounted(async () => {
  loadChecks(false);
  checksTimer = setInterval(() => loadChecks(false), 30000);
  stopChecks = await listen('checks', () => loadChecks(false));
});
onBeforeUnmount(() => {
  clearInterval(checksTimer);
  stopChecks?.();
});
</script>

<style scoped>
.play-screen { display: flex; height: 100%; }

/* The stage: night vision, the name and the controls. */
.stage { position: relative; flex: 1; min-width: 0; padding: 26px 36px; display: flex; flex-direction: column; justify-content: space-between; overflow: hidden;
  background: radial-gradient(60% 70% at 70% 35%, rgba(155, 224, 90, .10), transparent 70%), var(--bg); }
.glow { position: absolute; inset: -20%; background: radial-gradient(35% 35% at 70% 40%, rgba(155, 224, 90, .10), transparent 70%); animation: drift 14s ease-in-out infinite alternate; pointer-events: none; }
.scan { position: absolute; inset: 0; background: repeating-linear-gradient(0deg, rgba(155, 224, 90, .035) 0 1px, transparent 1px 4px); animation: scan 8s linear infinite; pointer-events: none; }
.hud { position: relative; display: flex; justify-content: space-between; align-items: center; min-height: 58px; }
.player { display: flex; align-items: center; gap: 12px; padding: 8px 18px 8px 8px; text-align: left; border-radius: 6px; background: rgba(12, 17, 18, .7); border: 1px solid var(--line); backdrop-filter: blur(8px); transition: border-color .2s; }
.player:hover { border-color: var(--control-line); }
.player b { display: block; font-weight: 600; }
.player small { font: 11px PlexMono, monospace; color: var(--muted); }
.avatar { width: 40px; height: 40px; border-radius: 50%; display: grid; place-items: center; color: var(--accent); font-weight: 600; border: 1.5px solid rgba(155, 224, 90, .5); background: var(--selected); }

.hero { position: relative; }
.hero .kicker { margin: 0; font-size: 12px; letter-spacing: .2em; }
h1 { margin: 4px 0 14px; font: 700 clamp(46px, 7.4vw, 92px)/.92 PlexCond, sans-serif; letter-spacing: .01em; text-transform: uppercase; }
h1 span { color: var(--accent); text-shadow: 0 0 40px rgba(155, 224, 90, .25); }
.tag { margin: 0 0 22px; color: var(--soft); font-size: 16px; }
.reveal { opacity: 0; transform: translateY(14px); animation: reveal .7s var(--ease) forwards; animation-delay: var(--d); }

.launch { display: flex; align-items: center; gap: 20px; max-width: 940px; }
.server { flex: 0 0 300px; display: flex; align-items: center; gap: 10px; padding: 10px 12px 10px 16px; text-align: left; border-radius: 4px; background: rgba(12, 17, 18, .8); border: 1px solid var(--control-line); transition: border-color .2s; backdrop-filter: blur(8px); }
.server:hover { border-color: var(--accent); }
.server > span:first-child { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.server b { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.chev { width: 16px; height: 16px; color: var(--muted); }
.ready { flex: 1; display: flex; align-items: center; gap: 10px; }
.ready i { width: 10px; height: 10px; border-radius: 50%; background: var(--muted); }
.ready i.ok { background: var(--accent); box-shadow: 0 0 8px var(--accent); }
.ready i.warn { background: var(--warn); animation: ping 2s ease-out infinite; }
.ready i.fail { background: var(--bad); animation: ping-bad 2s ease-out infinite; }
.play { position: relative; height: 58px; padding: 0 64px; background: var(--accent); color: var(--on-accent); font: 700 24px PlexCond, sans-serif; letter-spacing: .14em; text-transform: uppercase; overflow: hidden;
  clip-path: polygon(0 0, 100% 0, 100% 75%, calc(100% - 14px) 100%, 0 100%); transition: transform .12s, filter .2s; }
.play::after { content: ""; position: absolute; top: 0; bottom: 0; width: 40%; left: -60%; background: linear-gradient(100deg, transparent, rgba(255, 255, 255, .35), transparent); animation: sheen 4.5s ease-in-out infinite; }
.play:hover:not(:disabled) { filter: brightness(1.08); }
.play:active:not(:disabled) { transform: scale(.98); }
.play:disabled, .play-more:disabled { cursor: not-allowed; filter: saturate(.6) brightness(.85); }
.split { position: relative; display: flex; }
.play { clip-path: none; border-radius: 4px 0 0 4px; animation: pulse 2.8s ease-in-out infinite; }
.play:disabled { animation: none; }
.play-more { height: 58px; width: 42px; display: grid; place-items: center; background: var(--accent); color: var(--on-accent); border-left: 2px solid var(--bg); clip-path: polygon(0 0, 100% 0, 100% 75%, 75% 100%, 0 100%); }
.play-more svg { width: 18px; height: 18px; stroke-width: 2.4; }
.modes { position: absolute; right: 0; bottom: calc(100% + 10px); width: 300px; padding: 6px; display: flex; flex-direction: column; border-radius: 6px; background: rgba(12, 17, 18, .94); border: 1px solid var(--control-line); backdrop-filter: blur(14px); box-shadow: 0 18px 50px rgba(0, 0, 0, .55); z-index: 5; }
.modes button { display: flex; flex-direction: column; align-items: flex-start; padding: 10px 12px; border-radius: 4px; text-align: left; transition: background .15s; }
.modes button:hover { background: var(--control); }
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
.check small { margin-top: 2px; font: 12.5px/1.4 Plex, sans-serif; color: var(--muted); }
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
@keyframes reveal { to { opacity: 1; transform: none; } }
@keyframes drift { to { transform: translate(-6%, 4%) scale(1.08); } }
@keyframes scan { to { background-position: 0 64px; } }
@keyframes ping { 0% { box-shadow: 0 0 0 0 rgba(240, 180, 76, .5); } 70%, 100% { box-shadow: 0 0 0 9px rgba(240, 180, 76, 0); } }
@keyframes pulse { 50% { box-shadow: 0 0 34px rgba(155, 224, 90, .35); } }
@keyframes ping-bad { 0% { box-shadow: 0 0 0 0 rgba(255, 122, 107, .5); } 70%, 100% { box-shadow: 0 0 0 9px rgba(255, 122, 107, 0); } }
@keyframes spin { to { transform: rotate(360deg); } }
@keyframes sheen { 0%, 60% { left: -60%; } 100% { left: 130%; } }
@keyframes slide { from { transform: translateX(40px); opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .reveal { opacity: 1; transform: none; } }
</style>
