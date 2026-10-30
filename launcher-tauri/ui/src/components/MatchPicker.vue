<template>
  <Transition name="modal">
    <div v-if="open" class="backdrop" @click.self="$emit('close')">
      <div v-modal="() => $emit('close')" class="dialog picker" role="dialog" aria-modal="true" aria-labelledby="mp-title">
        <p class="kicker">Choose your match</p>
        <h2 id="mp-title">Open public matches</h2>
        <p class="soft">Pick a match to join, or start a new one of your own: matchmaking then skips every open match and you host.</p>

        <p v-if="!list" class="muted working"><i class="spin"></i>Looking at what's open…</p>
        <p v-else-if="!list.answered" class="muted">Your server didn't answer. <a href="#" @click.prevent="load">Try again</a></p>
        <template v-else>
          <p v-if="!list.rows.length" class="muted">No public matches are open right now: start one.</p>
          <ul v-else class="rows">
            <li v-for="r in list.rows" :key="r.room">
              <span class="what">
                <b>{{ r.mode }}</b>
                <small>{{ [r.map, r.players].filter(Boolean).join(' · ') }}</small>
              </span>
              <span v-if="r.started" class="mtag">Under way</span>
              <span v-else-if="r.full" class="mtag">Full</span>
              <button
                type="button"
                class="btn small"
                :disabled="r.started || r.full || busy"
                :aria-label="`Join ${r.mode}${r.map ? ' on ' + r.map : ''}, ${r.players}`"
                :title="r.started ? 'It has started: you can join when it\'s over' : r.full ? 'Every seat is taken' : null"
                @click="join(r.room)"
              >
                Join
              </button>
            </li>
          </ul>
          <p v-if="list.others" class="muted small">Also here: {{ list.others }}</p>
        </template>

        <div class="row end">
          <a v-if="list" href="#" class="again" @click.prevent="load">Refresh</a>
          <button type="button" class="btn" :disabled="busy" @click="start('coop')">New Co-op match</button>
          <button type="button" class="btn primary" :disabled="busy" @click="start('svm')">New SvM match</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup>
// Choosing your match (the 0.4.8 plan's L2): the open public matches on the player's server,
// each to join, or a brand-new one of their own (matchmaking then skips every open match and
// they host). For players avoiding someone who sits in a public lobby.
import { ref, watch } from 'vue';
import { call } from '../lib/tauri.js';

const props = defineProps({ open: Boolean });
const emit = defineEmits(['close']);

const list = ref(null);
const busy = ref(false);

async function load() {
  list.value = null;
  list.value = await call('matches');
}
watch(
  () => props.open,
  (open) => open && load(),
);

async function join(room) {
  busy.value = true;
  try {
    await call('join_match', { room });
    emit('close');
  } finally {
    busy.value = false;
  }
}

async function start(mode) {
  busy.value = true;
  try {
    await call('new_match', { mode });
    emit('close');
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped>
.backdrop { position: fixed; inset: 0; z-index: 40; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(600px, 100%); max-height: calc(100vh - 48px); overflow: auto; padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.dialog h2 { margin: 2px 0 10px; font: 700 26px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
.rows { list-style: none; margin: 14px 0 6px; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.rows li { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--line); }
.what { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.what b { font-weight: 600; }
.what small { color: var(--muted); font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.mtag { padding: 0 6px; border-radius: 3px; font: 10px/17px PlexMono, monospace; letter-spacing: .08em; text-transform: uppercase; border: 1px solid rgba(240, 180, 76, .4); color: var(--warn); }
.btn.small { height: 32px; padding: 0 14px; }
.btn:disabled { opacity: .55; cursor: not-allowed; }
.row { display: flex; align-items: center; gap: 10px; margin-top: 18px; }
.row.end { justify-content: flex-end; }
.again { margin-right: auto; font-size: 13px; }
.small { font-size: 12.5px; }
.working { display: flex; align-items: center; gap: 10px; margin-top: 14px; }
.spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
