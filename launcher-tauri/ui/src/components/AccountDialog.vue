<template>
  <Transition name="modal">
    <div v-if="asked" class="backdrop">
      <div v-modal="close" class="dialog" role="dialog" aria-modal="true" aria-labelledby="acc-title">
        <h2 id="acc-title">{{ server?.own ? `Connect to ${asked.server}` : 'Choose your name on this server' }}</h2>
        <div class="srv">
          <span>
            <b>{{ server?.place ?? asked.server }}</b>
            <small class="muted">{{ server?.kind }}{{ probe?.version ? ` · 5th Echelon ${probe.version}` : '' }}</small>
          </span>
          <span v-if="ping != null" class="ping" :class="ping <= 90 ? 'good' : ping <= 180 ? '' : 'far'">{{ ping }} ms</span>
          <i v-else-if="!probed" class="spin"></i>
          <span v-else class="muted">no answer</span>
        </div>
        <p class="soft">
          {{ asked.taken
            ? `Someone on this server already has the name ${asked.suggested} (an account from before names were kept for each player), so you need another one here. Your friends still see it's you.`
            : "You don't have an account here yet. Pick the name other players see. Your identity signs you in from now on: no password to remember." }}
        </p>
        <p v-if="probeError" class="warn">{{ probeError }}</p>
        <label class="label" for="acc-name">Your name</label>
        <input id="acc-name" ref="field" v-model="name" class="field" type="text" :maxlength="maxName" spellcheck="false" autocomplete="off" @input="typed" @keydown.enter="ready && create()" />
        <p v-if="name.trim()" class="check" :class="state.kind">
          <template v-if="state.kind === 'free'"><span class="mark ok"></span>Free</template>
          <template v-else-if="state.kind === 'taken'"><span class="mark fail"></span>Taken on this server</template>
          <template v-else-if="state.kind === 'not-allowed'"><span class="mark fail"></span>{{ state.why }}</template>
          <template v-else-if="state.kind === 'error'"><span class="muted small">Couldn't check it: {{ state.why }}</span></template>
          <template v-else-if="state.kind === 'checking'"><i class="spin"></i><span class="muted small">Checking…</span></template>
        </p>
        <div v-if="state.kind === 'taken' && suggestions.length" class="chips">
          <button v-for="s in suggestions" :key="s" type="button" @click="useName(s)">{{ s }}</button>
        </div>
        <p v-if="server?.own" class="muted small">This leaves {{ server.leaving }}: your friends and stats there stay there. The server menu on the Play screen takes you back.</p>
        <div class="actions">
          <button type="button" class="btn" @click="close">{{ !server?.own && server?.current ? `Stay on ${server.current}` : 'Cancel' }}</button>
          <button type="button" class="btn primary" :disabled="!ready" @click="create">{{ server?.own ? 'Create account and connect' : 'Create account' }}</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup>
// The account dialog (the egui launcher's, play.rs): a name for a new account on a server set up
// from outside the guided setup (a friend's, the server menu's, one of the player's own),
// checked with the server as it's typed, with free names offered when it's taken.
import { computed, nextTick, ref, watch } from 'vue';
import { call } from '../lib/tauri.js';

const props = defineProps({ asked: { type: Object, default: null }, maxName: { type: Number, default: 24 } });
const emit = defineEmits(['close']);

/** How long after the last key a name is checked. */
const CHECK_AFTER = 400;

const server = ref(null);
const probe = ref(null);
const probed = ref(false);
const probeError = ref(null);
const name = ref('');
const checks = ref({});
const suggestions = ref([]);
const field = ref(null);
let timer = null;
let suggestedFor = null;

const ping = computed(() => server.value?.ping ?? probe.value?.ping ?? null);
const state = computed(() => {
  const n = name.value.trim();
  if (checks.value[n]) return checks.value[n];
  return probe.value?.api ? { kind: 'checking' } : { kind: 'none' };
});
const ready = computed(() => name.value.trim() !== '' && !['taken', 'not-allowed'].includes(state.value.kind));

watch(
  () => props.asked,
  async (asked) => {
    if (!asked) return;
    name.value = asked.suggested;
    checks.value = asked.taken ? { [asked.suggested]: { kind: 'taken' } } : {};
    suggestions.value = [];
    suggestedFor = null;
    probe.value = null;
    probed.value = false;
    probeError.value = null;
    nextTick(() => field.value?.focus());
    server.value = await call('account_server', { server: asked.server, own: asked.own ?? null });
    try {
      probe.value = await call('account_probe', { server: asked.server });
    } catch (e) {
      probeError.value = String(e);
    }
    probed.value = true;
    check();
  },
  { immediate: true },
);

function typed() {
  clearTimeout(timer);
  timer = setTimeout(check, CHECK_AFTER);
}

async function check() {
  const n = name.value.trim();
  const api = probe.value?.api;
  if (!n || !api) return;
  if (!checks.value[n]) {
    try {
      checks.value = { ...checks.value, [n]: await call('check_name', { api, name: n }) };
    } catch (e) {
      checks.value = { ...checks.value, [n]: { kind: 'error', why: String(e) } };
    }
  }
  // Offers names when the one typed is taken.
  if (checks.value[n]?.kind === 'taken' && suggestedFor !== n) {
    suggestedFor = n;
    suggestions.value = [];
    suggestions.value = await call('name_suggestions', { api, name: n });
  }
}

function useName(s) {
  name.value = s;
  checks.value = { ...checks.value, [s]: { kind: 'free' } };
}

async function create() {
  const server = props.asked.server;
  emit('close');
  await call('connect', { server, newName: name.value.trim() });
}

function close() {
  emit('close');
}
</script>

<style scoped>
.backdrop { position: fixed; inset: 0; z-index: 48; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(520px, 100%); padding: 24px 26px 22px; display: flex; flex-direction: column; gap: 10px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
h2 { margin: 0; font: 700 22px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
p { margin: 0; }
.srv { display: flex; align-items: center; gap: 12px; padding: 10px 14px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.srv > span:first-child { flex: 1; display: flex; flex-direction: column; gap: 2px; }
.srv b { font-weight: 600; }
.srv small { font-size: 12.5px; }
.ping { font: 13px PlexMono, monospace; }
.good { color: var(--accent); } .far { color: var(--warn); }
.warn { color: var(--warn); font-size: 13.5px; }
.label { font-weight: 600; font-size: 14px; }
.field { height: 40px; padding: 0 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; font-weight: 600; font-size: 16px; }
.field:focus { border-color: var(--accent); }
.check { display: flex; align-items: center; gap: 8px; font-size: 13.5px; min-height: 20px; }
.check.free { color: var(--accent); } .check.taken, .check.not-allowed { color: var(--bad); }
.mark { width: 9px; height: 9px; border-radius: 50%; }
.mark.ok { background: var(--accent); } .mark.fail { background: var(--bad); }
.small { font-size: 12.5px; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chips button { height: 28px; padding: 0 12px; border-radius: 99px; background: var(--control); border: 1px solid var(--control-line); font-weight: 600; font-size: 13px; }
.chips button:hover { border-color: var(--accent); }
.actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 6px; }
.actions .btn { height: 38px; }
.btn:disabled { opacity: .45; cursor: default; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
</style>
