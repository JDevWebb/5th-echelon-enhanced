<template>
  <div class="setup">
    <div class="card wizard">
      <aside>
        <span class="lights" aria-hidden="true"><i></i><i></i><i></i></span>
        <p class="kicker">Operative setup</p>
        <h1 class="title">Get online in a minute</h1>
        <p class="soft">The launcher sets everything up and keeps the game's original files, so you can undo it any time. You need your own copy of the game.</p>
        <ol class="steps">
          <li v-for="(name, i) in STEPS" :key="name" :class="{ done: i < step, on: i === step }">
            <span class="box">
              <svg v-if="i < step" viewBox="0 0 12 12"><path d="M2.5 6.5l2.5 2.5 4.5-5.5" /></svg>
              <template v-else>{{ i + 1 }}</template>
            </span>
            {{ name }}
          </li>
        </ol>
      </aside>

      <section class="main">
        <p class="kicker">Step {{ step + 1 }} of {{ STEPS.length }}</p>
        <Transition name="view" mode="out-in">
          <!-- 1: the game -->
          <div v-if="step === 0" key="game">
            <h2 class="title">Find Splinter Cell: Blacklist</h2>
            <p v-if="finding" class="soft working"><i class="spin"></i>Looking in Steam, Ubisoft Connect and the usual folders…</p>
            <template v-else>
              <p class="soft">It wasn't in Steam, Ubisoft Connect or the usual folders. Choose the folder you installed it to.</p>
              <div class="row">
                <button type="button" class="btn primary big" @click="pickFolder">Choose folder…</button>
                <button type="button" class="btn big" @click="findGames">Look again</button>
              </div>
            </template>
          </div>

          <!-- 2: the server -->
          <div v-else-if="step === 1" key="server">
            <h2 class="title">Choose a server</h2>
            <p class="soft">Pick the closest server, or type a server's or a network's address. Connecting also installs the 5th Echelon client, keeping the game's original file so you can undo it.</p>
            <div class="found">
              <span class="dot"></span>
              <span><span>Splinter Cell: Blacklist found</span><small>{{ info.game_dir }}</small></span>
            </div>
            <fieldset :disabled="info.connecting">
              <p v-if="pinging && !rows.length" class="muted working"><i class="spin"></i>Pinging the servers…</p>
              <p v-else-if="servers && servers.network && !rows.length" class="muted">The directory lists no servers right now.</p>
              <div class="servers" role="radiogroup" aria-label="Servers">
                <button
                  v-for="r in rows"
                  :key="r.host"
                  type="button"
                  role="radio"
                  :aria-checked="server.trim() === r.host"
                  :class="{ chosen: server.trim() === r.host }"
                  @click="pick(r.host)"
                >
                  <span class="name">
                    <b>{{ r.title }}</b>
                    <small v-if="r.playing" class="soft">{{ r.playing }}</small>
                    <small class="muted">{{ r.host }} · {{ r.players }} online
                      <span v-if="r.best" class="badge good">Best for you</span>
                      <span v-else-if="r.friends_only" class="badge">Friends only</span>
                    </small>
                  </span>
                  <span class="ping" :class="pingClass(r.ping)">{{ r.ping == null ? 'no answer' : `${r.ping} ms` }}</span>
                </button>
              </div>
              <p class="caps label">Or a server's address</p>
              <div class="row">
                <input v-model="server" class="field address" type="text" autocomplete="off" placeholder="play.example.org or 192.168.1.20" spellcheck="false" @input="typed = true" />
                <template v-if="pinging || looking"><i class="spin"></i></template>
                <template v-else>
                  <button v-if="servers && servers.network" type="button" class="btn" @click="loadServers(true)">Ping again</button>
                  <button type="button" class="btn" @click="findOnNetwork">Find on my network</button>
                </template>
              </div>
              <div v-if="lan.length > 1" class="lan">
                <p class="muted">Several servers answered on this network. Choose the one you want:</p>
                <div class="row"><button v-for="ip in lan" :key="ip" type="button" class="btn" @click="pick(ip); lan = []">{{ ip }}</button></div>
              </div>
              <p v-if="servers && servers.note" class="muted small">{{ servers.note }}</p>
            </fieldset>
            <div class="row go">
              <p v-if="info.connecting" class="working"><i class="spin"></i>Connecting…</p>
              <button v-else type="button" class="btn primary big" :disabled="!server.trim()" @click="connect(null)">Connect</button>
              <button v-if="info.server && !info.connecting" type="button" class="btn big" @click="$emit('cancel')">Cancel</button>
            </div>
          </div>

          <!-- 3: the name -->
          <div v-else-if="step === 2" key="name">
            <h2 class="title">Pick your name</h2>
            <p class="soft">You don't have an account on {{ naming.server }} yet. Choose the name other players will see; your identity signs you in to it from now on.</p>
            <p v-if="naming.taken" class="warn">Someone on {{ naming.server }} already has that name: choose another.</p>
            <p class="caps label">Your name</p>
            <input v-model="nick" class="field name" type="text" autocomplete="off" placeholder="what other players see" :maxlength="info.max_name" spellcheck="false" :disabled="info.connecting" @keydown.enter="nick.trim() && connect(nick)" />
            <div class="row go">
              <p v-if="info.connecting" class="working"><i class="spin"></i>Creating your account…</p>
              <button v-else type="button" class="btn primary big" :disabled="!nick.trim()" @click="connect(nick)">Create account</button>
              <button v-if="info.server && !info.connecting" type="button" class="btn big" @click="naming = null; $emit('cancel')">Cancel</button>
            </div>
          </div>

          <!-- 4: the new game options, on a first setup -->
          <div v-else key="options">
            <h2 class="title">Game options</h2>
            <p class="soft">{{ INTRO_SETUP }}</p>
            <div class="options">
              <label v-for="(o, i) in options" :key="o.id" class="option" :class="{ blocked: o.blocked }">
                <span class="head">
                  <input v-model="choices[i]" type="checkbox" role="switch" class="switch" :disabled="!!o.blocked" />
                  <b>{{ o.title }}</b>
                  <span v-if="o.blocked" class="badge">{{ o.blocked[0] }}</span>
                  <span v-else-if="o.recommended" class="badge good">Recommended</span>
                </span>
                <span class="soft">{{ o.blocked ? o.blocked[1] : o.text }}</span>
              </label>
            </div>
            <p class="muted">{{ later }}</p>
            <div class="row go">
              <button type="button" class="btn primary big" @click="saveOptions">Continue</button>
            </div>
          </div>
        </Transition>
      </section>
    </div>
  </div>
</template>

<script setup>
// The guided setup (the egui launcher's, play.rs): find the game, choose a server, pick a name
// when there's no account there yet, and on a first setup the new game options. The setup
// itself (installing the client, the account, the settings) is the shared library's, run by
// the `connect` command; its steps show in the activity bar.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { call, listen } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true }, fresh: Boolean });
const emit = defineEmits(['changed', 'cancel', 'finished']);

const STEPS = ['Find the game', 'Choose a server', 'Pick your name', 'Game options', 'Play'];
const INTRO_SETUP = 'These change how the game plays. Each is set as we recommend; tick or untick any you like.';

const finding = ref(false);
const servers = ref(null);
const pinging = ref(false);
const looking = ref(false);
const lan = ref([]);
const server = ref('');
const typed = ref(false);
const naming = ref(null);
const nick = ref('');
const options = ref([]);
const choices = ref([]);
const later = ref('');

const rows = computed(() => servers.value?.rows ?? []);
const step = computed(() => {
  if (!props.info.game_dir) return 0;
  if (naming.value) return 2;
  if (!props.info.server) return 1;
  // Connected on a first setup: the options are its last step. Changing server: back to it.
  return props.fresh ? 3 : 1;
});

function pingClass(ms) {
  if (ms == null) return 'none';
  return ms <= 90 ? 'good' : ms <= 180 ? '' : 'far';
}

function pick(host) {
  server.value = host;
  typed.value = false;
}

async function findGames() {
  finding.value = true;
  const found = await call('find_games');
  if (found.length) await call('choose_game', { dir: found[0] });
  finding.value = false;
  emit('changed');
}

async function pickFolder() {
  if (await call('pick_game_folder')) emit('changed');
}

/** The network's servers, pinged; the best preselected unless the player typed one. */
async function loadServers(fresh) {
  pinging.value = true;
  servers.value = await call('servers', { fresh });
  pinging.value = false;
  const best = rows.value.find((r) => r.best);
  if (best && (!server.value.trim() || !typed.value)) server.value = best.host;
}

async function findOnNetwork() {
  looking.value = true;
  try {
    const ips = await call('find_on_network');
    lan.value = ips;
    if (ips.length === 1) pick(ips[0]);
  } catch {
    // The activity bar can't say this one: the server field stays as it was.
    lan.value = [];
  } finally {
    looking.value = false;
  }
}

async function connect(name) {
  const target = naming.value ? naming.value.server : server.value;
  if (await call('connect', { server: target, newName: name })) emit('changed');
}

async function loadOptions() {
  const q = await call('questions');
  options.value = q.options;
  choices.value = q.options.map((o) => o.on);
  later.value = q.options_later;
  if (!q.options.length) emit('finished');
}

async function saveOptions() {
  await call('answer_options', { choices: choices.value });
  emit('finished');
}

let stop = null;
onMounted(async () => {
  stop = await listen('setup', ({ payload }) => {
    if (payload.kind === 'needs-name') {
      naming.value = payload;
      nick.value = payload.suggested;
    } else if (payload.kind === 'ready') {
      naming.value = null;
      // Changing server is done once it worked; a first setup goes on to the options.
      if (!props.fresh) emit('finished');
    }
    emit('changed');
  });
  if (!props.info.game_dir) await findGames();
  else loadServers(false);
});
onBeforeUnmount(() => stop?.());

watch(() => props.info.game_dir, (dir) => dir && !servers.value && loadServers(false));
watch(step, (s) => s === 3 && loadOptions(), { immediate: true });
</script>

<style scoped>
.setup { height: 100%; overflow: auto; padding: 28px 16px; background: radial-gradient(60% 70% at 70% 35%, rgba(155, 224, 90, .07), transparent 70%), var(--bg); }
.wizard { max-width: 980px; margin: 0 auto; padding: 0; display: grid; grid-template-columns: 310px 1fr; overflow: hidden; }
aside { min-height: 534px; padding: 32px 30px; background: var(--sunken); border-right: 1px solid var(--line); }
aside .soft { margin: 6px 0 16px; }
aside .title { font-size: 34px; }
.lights { display: flex; gap: 5px; margin-bottom: 18px; }
.lights i { width: 7px; height: 7px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 8px var(--accent); }
.steps { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.steps li { position: relative; display: flex; align-items: center; gap: 14px; height: 40px; padding: 0 10px; margin: 0 -10px; border-radius: 4px; color: var(--muted); transition: background .25s, color .25s; }
.steps li.done { color: var(--soft); }
.steps li.on { color: var(--fg); font-weight: 600; background: var(--selected); box-shadow: inset 3px 0 0 var(--accent); }
.box { width: 24px; height: 24px; border-radius: 3px; display: grid; place-items: center; font: 12px PlexMono, monospace; border: 1.2px solid var(--control-line); }
.on .box { border-color: var(--accent); color: var(--accent); }
.done .box { border: 0; background: var(--selected); color: var(--accent); }
.box svg { width: 13px; height: 13px; stroke-width: 2.2; }
.main { padding: 32px 36px; min-width: 0; }
.main .title { font-size: 36px; margin-bottom: 6px; }
.main > div > .soft { margin: 0 0 14px; }
.found { display: flex; gap: 12px; align-items: center; padding: 12px 16px; margin-bottom: 14px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.found .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--accent); flex: none; }
.found small { display: block; font: 12px PlexMono, monospace; color: var(--muted); overflow-wrap: anywhere; }
fieldset { border: 0; margin: 0; padding: 0; min-width: 0; }
.servers { display: flex; flex-direction: column; gap: 6px; }
.servers button { display: flex; align-items: center; gap: 12px; padding: 12px 16px; text-align: left; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); transition: border-color .15s, background .15s; }
.servers button:hover { border-color: var(--control-line); }
.servers button.chosen { background: rgba(155, 224, 90, .08); border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
.name { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.name b { font-weight: 600; }
.name small { font-size: 12.5px; }
.ping { font: 13px PlexMono, monospace; color: var(--fg); }
.ping.good { color: var(--accent); } .ping.far { color: var(--warn); } .ping.none { color: var(--muted); font-family: Plex, sans-serif; }
.badge { display: inline-block; margin-left: 8px; padding: 0 7px; border-radius: 4px; font: 10.5px/18px PlexMono, monospace; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); border: 1px solid var(--control-line); vertical-align: 1px; }
.badge.good { color: var(--on-accent); background: var(--accent); border-color: var(--accent); }
.option .badge { margin-left: auto; color: var(--warn); border-color: rgba(240, 180, 76, .4); }
.option .badge.good { color: var(--accent); background: none; border-color: rgba(155, 224, 90, .4); }
.label { margin: 16px 0 6px; }
.row { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; }
.go { margin-top: 22px; }
.field { height: 40px; width: 320px; max-width: 100%; padding: 0 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; transition: border-color .15s; }
.field:focus { border-color: var(--accent); }
.field.address { flex: 1 1 220px; width: auto; }
.btn.big { height: 48px; padding: 0 28px; }
.btn.primary.big { min-width: 170px; }
.btn:disabled { opacity: .45; cursor: default; }
.lan { margin-top: 10px; }
.small { font-size: 12.5px; margin-top: 8px; }
.warn { color: var(--warn); }
.working { display: flex; align-items: center; gap: 10px; margin: 0; }
.spin { width: 16px; height: 16px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; flex: none; }
.options { display: flex; flex-direction: column; gap: 8px; margin: 4px 0 12px; }
.option { display: flex; flex-direction: column; gap: 4px; padding: 12px 16px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); cursor: pointer; }
.option.blocked { cursor: default; }
.head { display: flex; align-items: center; gap: 10px; }
.head b { font-weight: 600; }
.switch { appearance: none; flex: none; position: relative; width: 34px; height: 20px; margin: 0; border-radius: 10px; background: var(--control); border: 1px solid var(--control-line); cursor: pointer; transition: background .2s, border-color .2s; }
.switch::after { content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: var(--soft); transition: transform .2s var(--ease), background .2s; }
.switch:checked { background: var(--accent); border-color: var(--accent); }
.switch:checked::after { transform: translateX(14px); background: var(--on-accent); }
.switch:disabled { opacity: .5; }
@media (max-width: 760px) {
  .wizard { grid-template-columns: 1fr; }
  aside { border-right: 0; border-bottom: 1px solid var(--line); }
}
@keyframes spin { to { transform: rotate(360deg); } }
</style>
