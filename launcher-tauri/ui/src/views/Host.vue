<template>
  <div class="page">
    <div class="inner">
      <p class="kicker">Host on this PC</p>
      <h1 class="title">Host a server</h1>
      <p class="soft intro">Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.</p>

      <section v-if="view" class="card">
        <h3>Run a server on this PC</h3>
        <template v-if="!view.found">
          <p class="muted">The server ({{ view.exe_name }}) goes next to the launcher.</p>
          <div class="row">
            <p v-if="view.downloading" class="working"><i class="spin"></i>Downloading…</p>
            <button v-else type="button" class="btn primary" @click="download">Download the server</button>
            <a href="#" @click.prevent="call('open_url', { url: 'https://github.com/JDevWebb/5th-echelon-enhanced/releases' })">Releases</a>
          </div>
        </template>
        <p v-else-if="view.error" class="bad">{{ view.error }}</p>
        <template v-else>
          <fieldset :disabled="view.running || view.starting">
            <div class="grid">
              <span>Listen on</span>
              <select v-model="listen" class="select" aria-label="Listen on">
                <option value="">Every address</option>
                <option v-for="[name, ip] in view.adapters" :key="ip" :value="ip">{{ ip }} · {{ name }}</option>
              </select>
              <span>Public address</span>
              <input v-model="publicIp" class="field" type="text" placeholder="only behind NAT" autocomplete="off" />
              <span>Trusted subnet</span>
              <input
                v-model="trusted"
                class="field"
                type="text"
                placeholder="e.g. 10.8.0.0/16, the VPN players use"
                title="Players connecting from this subnet always advertise the address the server sees, fixing joins when the game picks the wrong adapter."
                autocomplete="off"
              />
            </div>
            <p class="muted">Community API (port 80, for launchers and the overlay):</p>
            <div v-if="api" class="row wrap">
              <label class="tick"><input v-model="api.info" type="checkbox" />Server info</label>
              <label class="tick" title="Shares every username and what online players are doing."><input v-model="api.presence" type="checkbox" />Who's online</label>
              <label class="tick" title="Lets tools create accounts through /api/register (rate-limited). Refused when every account needs an identity."><input v-model="api.accounts" type="checkbox" />Sign-up for tools</label>
              <label class="tick"><input v-model="api.unhandled" type="checkbox" />Unhandled calls (development)</label>
            </div>
          </fieldset>
          <div class="row">
            <p v-if="view.starting" class="working"><i class="spin"></i>Starting…</p>
            <template v-else-if="view.running">
              <span class="ok"><i></i>Running</span>
              <button type="button" class="btn" @click="stop">Stop</button>
            </template>
            <button v-else type="button" class="btn primary" @click="start">Start server</button>
          </div>
          <p v-if="startError" class="bad">{{ startError }}</p>
        </template>
      </section>

      <section v-if="view" class="card">
        <h3>Manage a server</h3>
        <template v-if="!view.managing">
          <p class="muted">Start the server above, or connect to one with the admin API on, using its key (admin-key.txt next to the server).</p>
          <div class="row">
            <input v-model="address" class="field short" type="text" placeholder="server address" autocomplete="off" />
            <input v-model="key" class="field" type="password" placeholder="admin key" autocomplete="off" />
            <button type="button" class="btn" :disabled="!address.trim() || !key.trim()" @click="connect(false)">Connect</button>
          </div>
          <p v-if="plain" class="row wrap">
            <span class="warn">{{ address }} isn't encrypted: anyone on the way can read the admin key. Use https:// if the server has it.</span>
            <button type="button" class="btn small" @click="connect(true)">Allow, then Connect</button>
          </p>
          <p v-if="connectError" class="bad">{{ connectError }}</p>
        </template>
        <template v-else>
          <div class="row">
            <span class="muted">{{ view.managing }}</span>
            <button type="button" class="btn small" @click="loadLists">Refresh</button>
            <button v-if="!view.local" type="button" class="btn small" @click="disconnect">Disconnect</button>
            <i v-if="loading" class="spin"></i>
          </div>
          <div class="tabs" role="tablist">
            <button v-for="[id, l] in [['players', 'Players'], ['games', 'Games'], ['logs', 'Log']]" :key="id" type="button" role="tab" :class="{ on: tab === id }" @click="tab = id">{{ l }}</button>
          </div>
          <p v-if="confirm" class="row wrap">
            <span class="warn">{{ confirm.what === 'player' ? `Delete the account ${confirm.name}? It can't be undone.` : `End ${confirm.name}?` }}</span>
            <button type="button" class="btn small" @click="remove">Yes</button>
            <button type="button" class="btn small" @click="confirm = null">No</button>
          </p>
          <template v-if="tab === 'players'">
            <p v-if="typeof lists?.players === 'string'" class="bad">{{ lists.players }}</p>
            <p v-else-if="!players.length" class="muted">No players yet.</p>
            <table v-else class="list">
              <tr v-for="p in players" :key="p.id"><td>{{ p.name }}</td><td class="muted">{{ p.ips }}</td><td class="end"><button type="button" class="btn small" @click="confirm = { what: 'player', id: p.id, name: p.name }">Delete…</button></td></tr>
            </table>
          </template>
          <template v-else-if="tab === 'games'">
            <p v-if="typeof lists?.games === 'string'" class="bad">{{ lists.games }}</p>
            <p v-else-if="!games.length" class="muted">No games running.</p>
            <table v-else class="list">
              <tr v-for="g in games" :key="g.id"><td>#{{ g.id }}</td><td>{{ g.kind }}</td><td class="muted">{{ g.who }}</td><td class="end"><button type="button" class="btn small" @click="confirm = { what: 'game', id: String(g.id), name: `game #${g.id}` }">End…</button></td></tr>
            </table>
          </template>
        </template>
        <template v-if="(!view.managing || tab === 'logs') && view.found">
          <div class="row">
            <h3>Log</h3>
            <select v-model="minLevel" class="select narrow" aria-label="Log detail">
              <option value="debug">Debug and above</option>
              <option value="info">Info and above</option>
              <option value="warn">Warn and above</option>
              <option value="error">Error and above</option>
            </select>
          </div>
          <div ref="logBox" class="log">
            <p v-for="(l, i) in logs" :key="i" :class="l.tone"><span class="muted mono">{{ l.time }}</span> {{ l.msg }}</p>
          </div>
        </template>
      </section>
    </div>
  </div>
</template>

<script setup>
// Host (the egui launcher's server.rs): run a 5th Echelon server on this PC, and manage a server
// (this one or another) through its admin API: players, games and the log.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { call } from '../lib/tauri.js';

const view = ref(null);
const listen = ref('');
const publicIp = ref('');
const trusted = ref('');
const api = ref(null);
const startError = ref(null);
const address = ref('');
const key = ref('');
const plain = ref(false);
const connectError = ref(null);
const tab = ref('players');
const lists = ref(null);
const loading = ref(false);
const confirm = ref(null);
const minLevel = ref('info');
const logs = ref([]);
const logBox = ref(null);
let timer = null;

const players = computed(() => (Array.isArray(lists.value?.players?.Ok) ? lists.value.players.Ok : []));
const games = computed(() => (Array.isArray(lists.value?.games?.Ok) ? lists.value.games.Ok : []));

async function load(form = false) {
  view.value = await call('host');
  if (form) {
    listen.value = view.value.listen ?? '';
    trusted.value = view.value.trusted_subnet;
    api.value = view.value.api;
  }
}

async function start() {
  startError.value = null;
  try {
    await call('host_start', { listen: listen.value || null, publicIp: publicIp.value, trustedSubnet: trusted.value, api: api.value });
    await load();
    loadLists();
  } catch (e) {
    startError.value = String(e);
    await load();
  }
}

async function stop() {
  await call('host_stop');
  lists.value = null;
  load();
}

async function download() {
  view.value.downloading = true;
  await call('host_download');
  load(true);
}

async function connect(allow) {
  connectError.value = null;
  try {
    const said = await call('host_connect', { address: address.value, key: key.value, allowPlain: allow });
    plain.value = said === 'plain';
    if (said === 'ok') {
      await load();
      loadLists();
    }
  } catch (e) {
    connectError.value = String(e);
  }
}

async function disconnect() {
  await call('host_disconnect');
  lists.value = null;
  load();
}

async function loadLists() {
  loading.value = true;
  lists.value = await call('host_lists');
  // The Rust side answers each list as Ok or Err.
  if (lists.value) {
    for (const k of ['players', 'games']) if (lists.value[k]?.Err) lists.value[k] = lists.value[k].Err;
  }
  loading.value = false;
}

async function remove() {
  const c = confirm.value;
  confirm.value = null;
  await call('host_delete', { what: c.what, id: c.id });
  loadLists();
}

async function loadLogs() {
  if (!view.value?.found) return;
  const box = logBox.value;
  const atBottom = !box || box.scrollHeight - box.scrollTop - box.clientHeight < 40;
  logs.value = await call('host_logs', { min: minLevel.value });
  if (atBottom) nextTick(() => logBox.value && (logBox.value.scrollTop = logBox.value.scrollHeight));
}

watch(minLevel, loadLogs);
onMounted(async () => {
  await load(true);
  if (view.value.managing) loadLists();
  loadLogs();
  timer = setInterval(() => {
    load();
    loadLogs();
  }, 3000);
});
onBeforeUnmount(() => clearInterval(timer));
</script>

<style scoped>
.intro { margin: 0 0 14px; }
.card { margin-bottom: 14px; display: flex; flex-direction: column; gap: 10px; }
.card p { margin: 0; }
h3 { margin: 0; font-size: 17px; font-weight: 600; }
fieldset { border: 0; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 10px; }
.grid { display: grid; grid-template-columns: max-content minmax(0, 320px); gap: 10px 12px; align-items: center; }
.row { display: flex; align-items: center; gap: 10px; }
.row.wrap { flex-wrap: wrap; }
.select, .field { height: 36px; padding: 0 10px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); color: var(--fg); outline: none; }
.select.narrow { width: auto; }
.field { flex: 1; max-width: 320px; }
.field.short { max-width: 200px; }
.field:focus, .select:focus { border-color: var(--accent); }
.tick { display: flex; align-items: center; gap: 8px; cursor: pointer; }
.tick input { accent-color: var(--accent); width: 16px; height: 16px; }
.ok { display: flex; align-items: center; gap: 8px; color: var(--accent); }
.ok i { width: 9px; height: 9px; border-radius: 50%; background: var(--accent); }
.bad { color: var(--bad); }
.warn { color: var(--warn); }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.btn:disabled { opacity: .45; cursor: default; }
.tabs { display: flex; gap: 4px; padding: 4px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); width: fit-content; }
.tabs button { height: 32px; padding: 0 14px; border-radius: 4px; color: var(--muted); }
.tabs button.on { background: var(--control); color: var(--fg); font-weight: 600; }
.list { border-collapse: collapse; width: 100%; }
.list td { padding: 6px 16px 6px 0; border-bottom: 1px solid var(--line); }
.list td.end { text-align: right; padding-right: 0; }
.log { max-height: 260px; overflow: auto; padding: 8px 10px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--line); font-size: 13px; user-select: text; }
.log p { margin: 0 0 2px; }
.log .warn { color: var(--warn); } .log .error { color: var(--bad); }
.mono { font: 12px PlexMono, monospace; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
