<template>
  <div class="page">
    <div class="inner">
      <p class="kicker">Network</p>
      <h1 class="title">Servers</h1>
      <p class="soft intro">Community servers are in the server menu on the Play screen. Here: a server of your own, a LAN party, or another network.</p>

      <p v-if="!info.game_dir" class="muted">Find the game first (Play).</p>
      <p v-else-if="info.managed" class="muted">Another tool manages this install: change the server there.</p>
      <template v-else>
        <section class="card">
          <p class="caps">Network</p>
          <div class="net">
            <span>
              <b class="big">{{ servers?.network ? host(servers.network) : 'No network' }}</b>
              <small class="muted">{{ networkLine }}</small>
            </span>
            <span v-if="servers?.network" class="inuse"><i></i>In use</span>
          </div>
          <hr />
          <p class="strong">Use another network</p>
          <div class="row">
            <input v-model="typedNetwork" class="field" type="text" placeholder="its address, e.g. play.mygroup.org" autocomplete="off" spellcheck="false" @keydown.enter="typedNetwork.trim() && lookUp()" />
            <i v-if="lookingUp" class="spin"></i>
            <button v-else type="button" class="btn" :disabled="!typedNetwork.trim()" @click="lookUp">Look it up</button>
          </div>
          <p v-if="networkError" class="bad small">{{ networkError }}</p>
          <p class="muted small">
            Another group's network of servers: its servers then fill the server menu on the Play screen.
            <a v-if="!community" href="#" @click.prevent="backToCommunity">Back to the community network</a>
          </p>
        </section>

        <div class="cols">
          <section class="card">
            <p class="caps">A server of your own</p>
            <div class="row">
              <input v-model="address" class="field" type="text" placeholder="play.example.org or 192.168.1.20" autocomplete="off" spellcheck="false" @keydown.enter="canConnect && own(address)" />
              <button type="button" class="btn primary" :disabled="!canConnect" :title="info.running ? 'Quit the game first' : null" @click="own(address)">Connect</button>
            </div>
            <p class="muted small">A private server for your group, outside the network: the server menu takes you back.</p>
            <template v-if="recent.length">
              <p class="muted small sub">Recent</p>
              <div v-for="r in recent" :key="r.address" class="srow">
                <b>{{ r.label }}</b><small class="muted">{{ r.when }}</small>
                <button type="button" class="btn small" :disabled="busy" @click="own(r.address)">Connect</button>
              </div>
            </template>
          </section>
          <section class="card">
            <p class="caps">On your network</p>
            <p class="soft">Looks for a server running at a LAN party, on this network.</p>
            <p v-if="looking" class="muted working"><i class="spin"></i>Looking…</p>
            <button v-else type="button" class="btn" @click="findLan">Find on my network</button>
            <p v-if="lanError" class="muted small">{{ lanError }}</p>
            <div v-for="ip in lan" :key="ip" class="srow">
              <b>{{ ip }}</b><small class="muted">found on your network</small>
              <button type="button" class="btn small" :disabled="busy" @click="own(ip)">Connect</button>
            </div>
          </section>
        </div>
      </template>

      <section class="card host">
        <p class="kicker">Host on this PC</p>
        <div class="row between">
          <p class="soft">Run a server for your group: a LAN party, or friends over the internet. Their launchers find it on your network.</p>
          <button type="button" class="btn" @click="$emit('go', 'host')">Host a server</button>
        </div>
      </section>
    </div>
  </div>
</template>

<script setup>
// The Servers screen (the egui launcher's, play.rs): the network the server menu shows and
// another to use, a server of the player's own (the account dialog confirms it), a LAN server,
// and the way to hosting one.
import { computed, onMounted, ref } from 'vue';
import { call } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true } });
const emit = defineEmits(['go', 'account', 'changed']);

const servers = ref(null);
const recent = ref([]);
const typedNetwork = ref('');
const lookingUp = ref(false);
const networkError = ref(null);
const address = ref('');
const looking = ref(false);
const lan = ref([]);
const lanError = ref(null);

const busy = computed(() => props.info.connecting || props.info.running);
const canConnect = computed(() => address.value.trim() !== '' && !busy.value);
const community = computed(() => servers.value?.community ?? true);
const networkLine = computed(() => {
  if (!servers.value?.network) return 'The server menu on the Play screen is empty until you use one.';
  const n = servers.value.rows.length;
  const count = n === 1 ? '1 server · ' : n ? `${n} servers · ` : '';
  return `${community.value ? 'The 5th Echelon community network · ' : ''}${count}your friends and identity follow you between its servers`;
});

const host = (url) => url.replace(/^https:\/\//, '').replace(/\/$/, '');

async function load() {
  servers.value = await call('servers', { fresh: false });
  recent.value = await call('recent_servers');
}

async function lookUp() {
  lookingUp.value = true;
  networkError.value = null;
  try {
    await call('use_network', { typed: typedNetwork.value });
    typedNetwork.value = '';
    await load();
    emit('changed');
  } catch (e) {
    networkError.value = String(e);
  } finally {
    lookingUp.value = false;
  }
}

async function backToCommunity() {
  await call('use_community_network');
  await load();
  emit('changed');
}

async function findLan() {
  looking.value = true;
  lanError.value = null;
  lan.value = [];
  try {
    lan.value = await call('find_on_network');
  } catch (e) {
    lanError.value = String(e);
  } finally {
    looking.value = false;
  }
}

/** A server of the player's own: the account dialog, which is also the confirmation. */
async function own(server) {
  const suggested = await call('usual_name');
  emit('account', { server: server.trim(), suggested, taken: false, own: true });
}

onMounted(load);
</script>

<style scoped>
.intro { margin: 0 0 14px; }
.card { margin-bottom: 14px; }
.card > p { margin: 0 0 6px; }
.net { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.net > span:first-child { display: flex; flex-direction: column; gap: 2px; }
.big { font-size: 18px; font-weight: 600; }
.small { font-size: 12.5px; }
.bad { color: var(--bad); }
.inuse { display: flex; align-items: center; gap: 6px; color: var(--accent); font-size: 13px; }
.inuse i { width: 8px; height: 8px; border-radius: 50%; background: var(--accent); }
hr { border: 0; border-top: 1px solid var(--line); margin: 12px 0; }
.strong { font-weight: 600; font-size: 14px; }
.row { display: flex; align-items: center; gap: 10px; margin: 6px 0; }
.row.between { justify-content: space-between; }
.row.between p { margin: 0; }
.field { flex: 1; max-width: 420px; height: 38px; padding: 0 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; }
.field:focus { border-color: var(--accent); }
.cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(340px, 1fr)); gap: 14px; }
.cols .card { margin: 0; }
.cols .field { max-width: none; }
.sub { margin: 10px 0 4px; }
.srow { display: flex; align-items: center; gap: 10px; margin-top: 4px; padding: 6px 6px 6px 12px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.srow b { font-weight: 600; }
.srow small { flex: 1; }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.btn:disabled { opacity: .45; cursor: default; }
.host { margin-top: 14px; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 16px; height: 16px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
