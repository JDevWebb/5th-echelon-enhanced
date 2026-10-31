<template>
  <div class="settings">
    <nav class="nav" aria-label="Settings">
      <p class="kicker">Loadout</p>
      <h1 class="title">Settings</h1>
      <button v-for="[id, label] in view?.sections ?? []" :key="id" type="button" :class="{ on: section === id }" :aria-current="section === id ? 'page' : null" @click="section = id">{{ label }}</button>
    </nav>

    <div class="page">
      <div v-if="view" class="inner">
        <h2 class="head">{{ label }}</h2>
        <p v-if="view.managed && locks" class="muted">{{ view.managed }} manages this install: server, account and network settings are read-only here.</p>
        <p v-if="!view.game && needsGame" class="muted">Find the game first (Settings › Game).</p>

        <Transition name="view" mode="out-in">
          <!-- Game -->
          <div v-if="section === 'game'" key="game">
            <section class="card">
              <h3>Game folder</h3>
              <p>{{ view.game?.dir ?? 'Not found yet.' }}</p>
              <div class="row">
                <select v-if="found.length > 1" class="select" aria-label="Other installs found" @change="choose($event.target.value)">
                  <option value="">Other installs found</option>
                  <option v-for="d in found" :key="d" :value="d">{{ d }}</option>
                </select>
                <button type="button" class="btn" @click="pickFolder">Choose folder…</button>
                <button v-if="view.game" type="button" class="btn" @click="act('open-game-folder')">Open folder</button>
              </div>
            </section>
            <template v-if="g">
              <section class="card">
                <h3>Options</h3>
                <template v-if="g.renderer">
                  <p class="label">Renderer</p>
                  <div class="seg">
                    <button type="button" :class="{ on: g.renderer === 'dx11' }" @click="set('renderer', 'dx11')">DirectX 11 (recommended)</button>
                    <button type="button" :class="{ on: g.renderer === 'dx9' }" @click="set('renderer', 'dx9')">DirectX 9</button>
                  </div>
                </template>
                <template v-if="g.through_steam !== null">
                  <Switch :on="g.through_steam" label="Start the game through Steam (for Steam Input and controllers)" @set="set('through_steam', $event)" />
                  <p v-if="g.through_steam" class="muted">Steam then picks DirectX 9 or 11 (its Play menu or launch options), not the Renderer above, and the launcher can't tell from its exit code whether the game crashed.</p>
                </template>
                <template v-if="g.proton !== null">
                  <p class="muted">Runs through Steam and Proton; Play asks Steam to start it.</p>
                  <template v-if="g.proton">
                    <p>This CPU needs these Steam launch options (Properties › Launch options) or the game can freeze at start:</p>
                    <p class="row"><code>{{ g.proton }}</code><button type="button" class="btn small" @click="copy(g.proton)">Copy</button></p>
                  </template>
                </template>
                <div class="row">
                  <Switch :on="g.overlay" label="In-game overlay, opened with" @set="set('overlay', $event)" />
                  <select class="select narrow" aria-label="Overlay key" :value="g.overlay_key" :disabled="!g.overlay" @change="set('overlay_key', $event.target.value)">
                    <option v-for="k in g.overlay_keys" :key="k" :value="k">{{ k }}</option>
                  </select>
                </div>
                <p v-if="g.overlay && /^F\d/.test(g.overlay_key)" class="muted">On most laptops, and some keyboards, hold Fn and press {{ g.overlay_key }}. If nothing happens, pick another key here.</p>
                <Switch :on="g.auto_join_invite" label="Join invites automatically" @set="set('auto_join_invite', $event)" />
                <template v-if="g.mouse_accel_blocked">
                  <Switch :on="false" disabled label="Remove negative mouse acceleration" />
                  <p class="muted">{{ g.mouse_accel_blocked }}</p>
                </template>
                <div v-else class="row">
                  <Switch
                    :on="g.disable_mouse_accel"
                    label="Remove negative mouse acceleration"
                    title="The game caps how far the view can turn in one frame, so fast flicks feel like the mouse slows down. This widens that cap for a linear, 1:1 feel (a wide limit stays, so a bad input spike can't flip the camera)."
                    @set="set('disable_mouse_accel', $event)"
                  />
                  <span class="kicker small">Recommended</span>
                </div>
                <Switch
                  :on="g.brighten"
                  :disabled="g.running"
                  label="Brighten the picture"
                  title="Blacklist caps its brightness low, which looks dark under Proton. This lifts the cap and sets a brighter picture by editing Blacklist.ini while the game is closed; turning it off puts the original back."
                  @set="set('brighten', $event)"
                />
                <label v-if="g.brighten" class="slider">
                  <span>Brightness</span>
                  <input v-model.number="brightness" type="range" :min="g.brightness_range[0]" :max="g.brightness_range[1]" :disabled="g.running" @change="set('brightness', brightness)" />
                  <span class="mono">{{ (brightness / 100).toFixed(2) }}</span>
                </label>
                <p v-if="g.running && g.brighten" class="muted">Close the game to change the picture brightness.</p>
                <Switch
                  :on="g.disable_autoclimb"
                  :disabled="g.running"
                  label="Disable automatic climbing (Active Sprint)"
                  title="The game climbs and vaults over edges by itself when you sprint into them. This turns that off, so climbing is only ever manual. Takes effect the next time the game starts."
                  @set="set('disable_autoclimb', $event)"
                />
                <p v-if="g.running" class="muted">Close the game to change automatic climbing.</p>
                <div class="row">
                  <span>Client log detail</span>
                  <select class="select narrow" aria-label="Client log detail" :value="g.log_level" @change="set('log_level', $event.target.value)">
                    <option v-for="l in g.log_levels" :key="l" :value="l">{{ l }}</option>
                  </select>
                </div>
                <div class="row">
                  <span>Extra command line</span>
                  <input v-model="commandLine" class="field" type="text" placeholder="Unreal Engine options" autocomplete="off" @change="set('command_line', commandLine)" />
                </div>
              </section>
              <section class="card">
                <h3>Beta features</h3>
                <p class="warn">Experimental: these may feel wrong, stutter, or crash the game. Turn one off if it gives you trouble.</p>
                <p class="muted">While any of them is on, the launcher sends the game's full log (names, folders and addresses on your PC hidden) to your server's admins each time the game closes, so they can see how it went. Each takes effect the next time the game starts.</p>
                <Switch v-for="[l, hover] in g.beta_features" :key="l" :on="g.mouse_turn_per_second" :label="l" :title="hover" @set="set('mouse_turn_per_second', $event)" />
                <p v-if="g.mouse_turn_per_second" class="accent">Beta on: your game's log is sent after each game.</p>
              </section>
            </template>
          </div>

          <!-- Network -->
          <div v-else-if="section === 'network' && n" key="network">
            <section class="card" :class="{ locked: view.managed }">
              <h3>Internet play</h3>
              <p class="muted">How other players reach this PC.</p>
              <div class="seg">
                <button v-for="[id, l] in NAT" :key="id" type="button" :class="{ on: n.nat === id }" :disabled="!!view.managed" @click="set('nat', id)">{{ l }}</button>
              </div>
              <p class="muted">{{ NAT_SAYS[n.nat] }}</p>
              <Switch
                :on="n.port_mapping"
                :disabled="n.nat === 'off' || !!view.managed"
                label="Open the game's port on the router (UPnP / NAT-PMP)"
                title="Asks the router to forward UDP 13000 to this PC while the game runs, so other players can always reach you directly."
                @set="set('port_mapping', $event)"
              />
            </section>
            <section v-if="relays?.list.length" class="card">
              <h3>Relay</h3>
              <p class="muted">Your server works with relays near its players: matches with players who can't reach each other directly go through one. The nearest is used unless you choose one.</p>
              <div class="row">
                <span>Relay</span>
                <select class="select" aria-label="Relay" :value="relays.chosen ?? ''" @change="chooseRelay($event.target.value || null)">
                  <option value="">The nearest{{ nearestName ? ` (now ${nearestName})` : '' }}</option>
                  <option v-for="r in relays.list" :key="r.id" :value="r.id">{{ r.name }}{{ r.region ? ` · ${r.region}` : '' }} · {{ r.ms == null ? 'no answer' : `${r.ms} ms` }}</option>
                </select>
              </div>
              <p class="muted small">Used from the next time the game starts.</p>
            </section>
            <section class="card">
              <h3>Connection test</h3>
              <p class="muted">Checks each part of the connection in turn: the server's config, its API, signing in to the game service, how other players reach this PC (as the overlay shows it in game), and the server's helper for playing over the internet.</p>
              <p v-if="testing" class="working"><i class="spin"></i>Testing…</p>
              <button v-else type="button" class="btn" @click="test">Run the test</button>
              <p v-if="testError" class="muted">{{ testError }}</p>
              <p v-for="[name, status, note] in tests" :key="name" class="result"><span class="mark" :class="status"></span>{{ name }}<small v-if="note" class="muted">{{ note }}</small></p>
              <p v-if="tests.some(([, s]) => s !== 'ok') && g?.running" class="muted">The game is running and holds the ports some tests need; close it and test again.</p>
            </section>
            <section class="card">
              <h3>Network adapter</h3>
              <p class="muted">The game uses one network adapter for matches: automatically, the one that reaches your server, chosen each time the game starts. Pin one only if your group plays over a VPN on purpose.</p>
              <div class="row">
                <span>Adapter</span>
                <select class="select" aria-label="Adapter" :value="n.adapter ?? ''" :disabled="!!view.managed" @change="set('adapter', $event.target.value || null)">
                  <option value="">{{ n.automatic }}</option>
                  <option v-for="[name, ip] in n.adapters" :key="name" :value="name">{{ name }} · {{ ip }}</option>
                </select>
              </div>
              <Switch v-if="n.adapter" :on="n.require_adapter" :disabled="!!view.managed" label="Don't start the game without this adapter" title="Stops the game from quietly playing over the wrong network when your VPN is off." @set="set('require_adapter', $event)" />
            </section>
          </div>

          <!-- Servers and accounts -->
          <div v-else-if="section === 'servers' && view.game" key="servers">
            <section class="card">
              <p v-if="!view.profiles.length" class="muted">None yet. Join one on the Play screen.</p>
              <template v-else>
                <label class="tick"><input v-model="showPasswords" type="checkbox" @change="loadPasswords" />Show passwords</label>
                <table class="profiles">
                  <tr v-for="p in view.profiles" :key="p.name">
                    <td :class="{ accent: p.current }">{{ p.server }}</td>
                    <td>
                      <input v-if="renaming?.name === p.name" v-model="renaming.to" class="field" type="text" placeholder="new name" autocomplete="off" />
                      <template v-else>{{ p.username }}</template>
                    </td>
                    <td class="mono">{{ showPasswords ? passwords[p.name] ?? '…' : '••••••••' }}</td>
                    <td class="row end">
                      <template v-if="renaming?.name === p.name">
                        <button type="button" class="btn small" :disabled="!renaming.to.trim()" @click="rename">Save name</button>
                        <button type="button" class="btn small" @click="renaming = null">Cancel</button>
                      </template>
                      <template v-else>
                        <button type="button" class="btn small" :disabled="!!view.managed" title="A new name on this server; friends and invites carry on" @click="renaming = { name: p.name, to: p.username }">Rename</button>
                        <button v-if="!p.current" type="button" class="btn small" :disabled="!!view.managed" @click="set('use_profile', p.name)">Use</button>
                        <button type="button" class="btn small" :disabled="!!view.managed" @click="set('remove_profile', p.name)">Remove</button>
                      </template>
                    </td>
                  </tr>
                </table>
              </template>
            </section>
          </div>

          <!-- Identity and friends -->
          <div v-else-if="section === 'identity'" key="identity">
            <section class="card">
              <p class="muted">Your identity follows you between servers that share friends: friends you make on one show up on the others. It also signs you in to your accounts on a new PC.</p>
              <p v-if="identity?.short" class="row">Your identity <b>{{ identity.short }}</b><button type="button" class="btn small" @click="exportIdentity">Copy to move it to another PC</button></p>
              <p v-else-if="identity?.error" class="bad">Your identity can't be read: {{ identity.error }}</p>
              <p v-else-if="identity" class="muted">None yet: it's made when you join a server that supports it.</p>
              <p class="muted">On a new PC, import your identity before you connect to a server: connecting first makes a new identity, and a new account under another name.</p>
              <div class="row">
                <span>Import</span>
                <input v-model="importText" class="field" type="password" placeholder="5th-echelon-identity:v1:…" autocomplete="off" @input="confirmImport = false" />
                <button type="button" class="btn" :disabled="!importText.trim() || confirmImport" @click="importIdentity(false)">Use this identity</button>
              </div>
              <p v-if="importError" class="bad small">{{ importError }}</p>
              <div v-if="confirmImport" class="danger">
                <p>This replaces {{ identity?.short ?? "this PC's identity" }}.{{ identity?.accounts === 1 ? ' Your saved account here is linked to it.' : identity?.accounts > 1 ? ` Your ${identity.accounts} saved accounts here are linked to it.` : '' }} Accounts made with it can only be signed in to with it: if you'll want them, copy it first. A copy of it stays in the launcher's folder.</p>
                <p class="row"><button type="button" class="btn small" @click="importIdentity(true)">Replace it</button><button type="button" class="btn small" @click="confirmImport = false">Cancel</button></p>
              </div>
              <hr />
              <div class="row">
                <span>Server directory</span>
                <input v-model="directory" class="field" type="text" placeholder="https://coordinator.example.com" autocomplete="off" />
                <button v-if="directory !== view.directory" type="button" class="btn small" @click="saveDirectory">Save</button>
              </div>
              <p v-if="directoryError" class="bad small">{{ directoryError }}</p>
              <p class="muted">The servers to choose from: the community network unless you join another network. Empty: none.</p>
            </section>
          </div>

          <!-- Notifications -->
          <div v-else-if="section === 'notifications' && view.notifications" key="notifications">
            <section class="card">
              <h3>Chat in the game</h3>
              <p class="muted">When a chat message pops up in the corner of the game. Messages that name you (@name) count as mentions. Invitations and friend requests always pop up.</p>
              <div class="grid">
                <template v-for="([key, l], i) in [['notify_global', 'Global'], ['notify_lobby', 'Lobby'], ['notify_direct', 'Messages from friends']]" :key="key">
                  <span>{{ l }}</span>
                  <select class="select" :aria-label="l" :value="view.notifications[i]" @change="set(key, $event.target.value)">
                    <option v-for="[id, t] in view.toasts" :key="id" :value="id">{{ t }}</option>
                  </select>
                </template>
              </div>
              <p class="muted small">"Outside matches" keeps Global quiet while you play a match; it still tells you when someone names you.</p>
            </section>
          </div>

          <!-- Save game -->
          <div v-else-if="section === 'save' && view.save" key="save">
            <section class="card">
              <h3>Save game</h3>
              <p v-if="view.save.no_folder" class="muted">{{ view.save.no_folder }}</p>
              <template v-else>
                <p>{{ view.save.path }}</p>
                <p class="muted">{{ view.save.state }}</p>
                <div class="row wrap">
                  <button v-if="!view.save.missing" type="button" class="btn" @click="act('backup-save')">Back up now</button>
                  <button v-if="view.save.ubisoft" type="button" class="btn" :title="view.save.ubisoft" @click="act('import-ubisoft')">Import from Ubisoft Connect</button>
                  <button type="button" class="btn" title="A Blacklist save from another PC, Ubisoft Connect's 1.save, or one of the launcher's backups. Your current save is backed up first." @click="act('import-save')">Import a save file…</button>
                  <button type="button" class="btn" @click="confirmSave = true">New rank 5 save…</button>
                  <button type="button" class="btn" @click="act('open-save-folder')">Open folder</button>
                </div>
                <p v-if="confirmSave" class="row warnline">
                  <span class="warn">Replace your save with a new rank 5 one? The current save is backed up first.</span>
                  <button type="button" class="btn small" @click="confirmSave = false; act('new-save')">Replace</button>
                  <button type="button" class="btn small" @click="confirmSave = false">Cancel</button>
                </p>
              </template>
            </section>
          </div>

          <!-- 5th Echelon client -->
          <div v-else-if="section === 'client' && view.client" key="client">
            <section class="card">
              <h3>5th Echelon client</h3>
              <p class="muted">{{ view.client.state }}</p>
              <p v-if="view.client.version" class="muted small">Carried by this launcher: {{ view.client.version }}</p>
              <div class="row">
                <button v-if="view.client.can_install" type="button" class="btn" @click="act('install')">Install</button>
                <button v-if="view.client.can_uninstall" type="button" class="btn" @click="confirmUninstall = true">Uninstall…</button>
              </div>
              <template v-if="confirmUninstall">
                <p class="warn">Put the game's own online DLL back? The game then won't connect to 5th Echelon servers. Your 5th Echelon settings, accounts, saves and identity stay on this PC, so Install brings it all back.</p>
                <p class="row"><button type="button" class="btn small" @click="confirmUninstall = false; act('uninstall')">Uninstall</button><button type="button" class="btn small" @click="confirmUninstall = false">Cancel</button></p>
              </template>
            </section>
          </div>

          <!-- Advanced -->
          <div v-else-if="section === 'advanced' && view.game" key="advanced">
            <section class="card">
              <h3>Hooks</h3>
              <p class="muted">Which parts of the game the client patches. Leave these alone unless you're debugging.</p>
              <label class="tick"><input type="checkbox" :checked="view.hooks_all" @change="set('hooks_all', $event.target.checked)" />All hooks</label>
              <div class="hooks" :class="{ off: view.hooks_all }">
                <label v-for="[id, l, on] in view.hooks" :key="id" class="tick"><input type="checkbox" :checked="on" :disabled="view.hooks_all" @change="set('hook', [id, $event.target.checked])" />{{ l }}</label>
              </div>
              <p class="muted">For working out why two copies of the game refuse each other ("a different version of the game"): the game writes what its data version is made of to bl-dataversion.txt in its folder. Off, the file is deleted.</p>
              <label class="tick"><input type="checkbox" :checked="view.log_data_version" @change="set('log_data_version', $event.target.checked)" />Write the game's data version (bl-dataversion.txt)</label>
            </section>
          </div>

          <!-- Display -->
          <div v-else-if="section === 'display'" key="display">
            <section class="card">
              <h3>Size</h3>
              <p class="muted">The launcher scales with its window: resize it, maximise it, or press F11 for full screen. Make everything bigger or smaller here, or with Ctrl + and Ctrl −; Ctrl 0 puts it back.</p>
              <div class="row">
                <button type="button" class="btn" :disabled="size <= view.size_range[0] + 0.01" title="Smaller (Ctrl −)" @click="$emit('size', size - 0.1)">−</button>
                <b>{{ Math.round(size * 100) }} %</b>
                <button type="button" class="btn" :disabled="size >= view.size_range[1] - 0.01" title="Bigger (Ctrl +)" @click="$emit('size', size + 0.1)">+</button>
                <button v-if="Math.abs(size - 1) > 0.01" type="button" class="btn" title="Ctrl 0" @click="$emit('size', 1)">Reset</button>
              </div>
              <button type="button" class="btn" @click="$emit('fullscreen')">Full screen (F11)</button>
            </section>
            <section v-if="view.classic !== null" class="card">
              <h3>Interface</h3>
              <p class="muted">The classic interface is the launcher as it was before 0.4.8: plainer, and it runs where this one can't (it's always used in the Steam Deck's gaming mode). Settings › Display there switches back.</p>
              <button type="button" class="btn" @click="call('use_classic')">Use the classic interface</button>
            </section>
          </div>

          <!-- Feedback -->
          <div v-else-if="section === 'feedback'" key="feedback">
            <section v-if="view.game" class="card">
              <h3>While you play</h3>
              <p class="muted">The game sends the server it's signed in to its warnings, errors and network events as they happen, and what it's doing (the game session it's in, its checkpoint saves, which of 5th Echelon's fixes went in and what they did), so the server's admins can see why a join, a connection or a mission failed. Your PC's name, your user folder and your internet address are hidden first.</p>
              <Switch :on="view.send_diagnostics" label="Send the game's diagnostics to the server" @set="set('send_diagnostics', $event)" />
              <p class="muted">With it on, when the server sees a problem in your game (a join that failed, your connection dropping, the game restarting in a match), it can also ask for the 15 minutes of the game's log before it, at most once an hour (never the whole log), private details hidden the same way.</p>
            </section>
            <section class="card">
              <h3>After a game</h3>
              <p class="muted">When something goes wrong in a game (a join that fails, a crash), and now and then otherwise, the launcher asks how it went once the game closes. Your answer goes to the server's admins, with your logs if you agree: your PC's name, your user folder and your internet address are hidden first.</p>
              <Switch :on="view.ask_feedback" label="Ask me how my games went" @set="set('ask_feedback', $event)" />
            </section>
            <section class="card">
              <h3>Report a problem</h3>
              <p class="muted">Something wrong now? Tell the admins of the server you play on, with your logs.</p>
              <button type="button" class="btn" @click="call('ask_feedback')">Send feedback</button>
            </section>
            <section class="card">
              <h3>Your reports</h3>
              <p class="muted">What you sent, and what the admins replied.</p>
              <p v-if="reports === undefined" class="working"><i class="spin"></i></p>
              <p v-else-if="reports === null" class="muted">Connect to a server first: your reports are read with the identity made then.</p>
              <p v-else-if="reportsError" class="muted">{{ reportsError }}</p>
              <p v-else-if="!reports.length" class="muted">None yet.</p>
              <div v-for="r in reports ?? []" :key="r.line + r.said" class="report">
                <p>{{ r.line }}</p>
                <p class="muted">{{ r.said }}</p>
                <p v-if="r.reply" class="accent">Reply: {{ r.reply }}</p>
              </div>
              <a href="#" @click.prevent="loadReports">Check again</a>
            </section>
          </div>

          <!-- About -->
          <div v-else-if="section === 'about'" key="about">
            <section class="card">
              <h3>About</h3>
              <p>{{ view.product }} {{ view.release }}</p>
              <p class="muted small">Developed by JDevWebb, building on 5th Echelon by unixoide and its contributors.</p>
              <p class="muted small">Free, and provided as is, without warranty of any kind. Not affiliated with or endorsed by Ubisoft.</p>
              <div class="row">
                <i v-if="checking" class="spin"></i>
                <button v-else type="button" class="btn" @click="checkUpdate">Check for updates</button>
                <span v-if="update" :class="update[1] ? 'accent' : 'muted'">{{ update[1] ? `Version ${update[0]} is available.` : `Up to date (latest release ${update[0]}).` }}</span>
                <span v-if="updateError" class="muted">{{ updateError }}</span>
                <a href="#" @click.prevent="call('open_url', { url: 'https://github.com/JDevWebb/5th-echelon-enhanced/releases' })">Releases</a>
              </div>
            </section>
          </div>
        </Transition>
      </div>
    </div>
  </div>
</template>

<script setup>
// Settings (the egui launcher's settings.rs): its sections down the side, one shown at a time,
// worded as egui's. Each control changes one setting through `set`; the work some start (a save
// imported, the client installed) shows in the activity bar.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { call, listen } from '../lib/tauri.js';
import Switch from '../components/Switch.vue';

const props = defineProps({ open: { type: String, default: null }, size: { type: Number, default: 1 } });
const emit = defineEmits(['size', 'fullscreen', 'changed']);

const NAT = [['auto', 'Automatic'], ['relay', 'Always through the server'], ['off', 'LAN or VPN only']];
const NAT_SAYS = {
  auto: 'The server tells the game this PC’s public address, so players connect directly; when your router can’t be reached, play goes through the server.',
  relay: 'All match traffic goes through the server. Use it when direct connections fail; it adds a little delay.',
  off: 'The game advertises this PC’s local address, as it always did: other players must be on your network or VPN.',
};

const view = ref(null);
const section = ref(props.open ?? 'game');
const found = ref([]);
const brightness = ref(100);
const commandLine = ref('');
const tests = ref([]);
const testing = ref(false);
const testError = ref(null);
const showPasswords = ref(false);
const passwords = ref({});
const renaming = ref(null);
const identity = ref(null);
const importText = ref('');
const importError = ref(null);
const confirmImport = ref(false);
const directory = ref('');
const directoryError = ref(null);
const confirmSave = ref(false);
const confirmUninstall = ref(false);
const reports = ref(undefined);
const reportsError = ref(null);
const checking = ref(false);
const update = ref(null);
const updateError = ref(null);
let stop = null;

const g = computed(() => view.value?.game);
const n = computed(() => view.value?.network);
const label = computed(() => view.value?.sections.find(([id]) => id === section.value)?.[1] ?? '');
// The sections that need the game, and those another tool's management locks.
const needsGame = computed(() => !['game', 'identity', 'display', 'feedback', 'about'].includes(section.value));
const locks = computed(() => ['network', 'servers'].includes(section.value));

async function load() {
  view.value = await call('settings');
  brightness.value = view.value.game?.brightness ?? 100;
  commandLine.value = view.value.game?.command_line ?? '';
  directory.value = view.value.directory;
}

async function set(key, value) {
  await call('set', { key, value });
  await load();
  emit('changed');
}

async function act(action, arg = null) {
  await call('act', { action, arg });
}

async function choose(dir) {
  if (dir && (await call('choose_game', { dir }))) {
    await load();
    emit('changed');
  }
}

async function pickFolder() {
  if (await call('pick_game_folder')) {
    await load();
    emit('changed');
  }
}

async function copy(text) {
  await navigator.clipboard.writeText(text).catch(() => {});
}

async function test() {
  testing.value = true;
  testError.value = null;
  try {
    tests.value = await call('connection_test');
  } catch (e) {
    testError.value = String(e);
  } finally {
    testing.value = false;
  }
}

async function loadPasswords() {
  if (!showPasswords.value) return;
  for (const p of view.value.profiles) passwords.value[p.name] = await call('profile_password', { name: p.name });
}

async function rename() {
  const r = renaming.value;
  renaming.value = null;
  await act('rename', [r.name, r.to]);
}

async function exportIdentity() {
  const text = await call('export_identity');
  if (text) await copy(text);
}

async function importIdentity(confirm) {
  importError.value = null;
  try {
    const said = await call('import_identity', { text: importText.value, confirm });
    if (said === 'confirm') confirmImport.value = true;
    else {
      importText.value = '';
      confirmImport.value = false;
      identity.value = await call('identity');
    }
  } catch (e) {
    importError.value = String(e);
  }
}

async function saveDirectory() {
  directoryError.value = null;
  try {
    await call('set_directory', { url: directory.value });
    await load();
    emit('changed');
  } catch (e) {
    directoryError.value = String(e);
  }
}

async function loadReports() {
  reports.value = undefined;
  reportsError.value = null;
  try {
    reports.value = await call('my_reports');
  } catch (e) {
    reports.value = [];
    reportsError.value = String(e);
  }
}

async function checkUpdate() {
  checking.value = true;
  updateError.value = null;
  try {
    update.value = await call('check_update');
  } catch (e) {
    updateError.value = String(e);
  } finally {
    checking.value = false;
  }
}

// The relays the server offers (only a network's matchmaking server does), pinged when the
// Network section opens.
const relays = ref(null);
const nearestName = computed(() => (relays.value?.chosen ? null : relays.value?.list.find((r) => r.id === relays.value.in_use)?.name));
async function chooseRelay(id) {
  await call('choose_relay', { relay: id });
  relays.value = await call('relays');
}

watch(section, (s) => {
  if (s === 'network') call('relays').then((r) => (relays.value = r));
  if (s === 'identity') call('identity').then((i) => (identity.value = i));
  if (s === 'feedback') loadReports();
  if (s === 'game') call('find_games').then((f) => (found.value = f));
});
watch(() => props.open, (s) => s && (section.value = s));

onMounted(async () => {
  await load();
  found.value = await call('find_games');
  stop = await listen('settings', load);
  if (section.value === 'identity') identity.value = await call('identity');
  if (section.value === 'feedback') loadReports();
});
onBeforeUnmount(() => stop?.());
</script>

<style scoped>
.settings { display: flex; height: 100%; }
.nav { width: 230px; flex: none; display: flex; flex-direction: column; gap: 2px; padding: 26px 16px 16px; background: var(--surface); border-right: 1px solid var(--line); overflow: auto; }
.nav .title { font-size: 34px; margin-bottom: 12px; }
.nav button { position: relative; min-height: 40px; padding: 0 12px; border-radius: 4px; text-align: left; color: var(--soft); transition: background .15s, color .15s; }
.nav button:hover { background: var(--control); }
.nav button.on { background: var(--selected); color: var(--fg); font-weight: 600; }
.nav button.on::before { content: ""; position: absolute; left: 0; top: 8px; bottom: 8px; width: 2px; border-radius: 1px; background: var(--accent); }
.page { flex: 1; min-width: 0; }
.head { margin: 0 0 10px; font-size: 22px; font-weight: 600; }
.card { margin-bottom: 10px; display: flex; flex-direction: column; gap: 8px; }
.card p { margin: 0; }
.card > .btn, .card > a, .card > label { align-self: flex-start; }
h3 { margin: 0 0 2px; font-size: 17px; font-weight: 600; }
.label { font-size: 14px; }
.small { font-size: 12.5px; }
.mono, code { font: 13px PlexMono, monospace; }
.accent { color: var(--accent); }
.warn { color: var(--warn); }
.bad { color: var(--bad); }
.row { display: flex; align-items: center; gap: 10px; }
.row.wrap { flex-wrap: wrap; }
.row.end { justify-content: flex-end; }
.seg { display: flex; flex-wrap: wrap; gap: 4px; padding: 4px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); width: fit-content; }
.seg button { height: 34px; padding: 0 14px; border-radius: 4px; color: var(--muted); }
.seg button.on { background: var(--control); color: var(--fg); font-weight: 600; }
.seg button:disabled { cursor: default; }
.select, .field { height: 36px; padding: 0 10px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); color: var(--fg); outline: none; }
.select { min-width: 260px; }
.select.narrow { min-width: 110px; }
.field { flex: 1; max-width: 420px; }
.field:focus, .select:focus { border-color: var(--accent); }
.slider { display: flex; align-items: center; gap: 12px; }
.slider input { width: 260px; accent-color: var(--accent); }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.btn:disabled { opacity: .45; cursor: default; }
.tick { display: flex; align-items: center; gap: 10px; cursor: pointer; }
.tick input { accent-color: var(--accent); width: 16px; height: 16px; }
.result { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
.mark { width: 9px; height: 9px; border-radius: 50%; background: var(--accent); flex: none; align-self: center; }
.mark.warn { background: var(--warn); } .mark.fail { background: var(--bad); }
.profiles { border-collapse: collapse; width: 100%; }
.profiles td { padding: 8px 16px 8px 0; border-bottom: 1px solid var(--line); }
.grid { display: grid; grid-template-columns: max-content 260px; gap: 8px 16px; align-items: center; }
.hooks { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 4px 12px; }
.hooks.off { opacity: .5; }
.danger { padding: 12px 14px; border-radius: 6px; background: rgba(255, 122, 107, .10); display: flex; flex-direction: column; gap: 8px; }
.warnline { flex-wrap: wrap; }
hr { width: 100%; border: 0; border-top: 1px solid var(--line); margin: 6px 0; }
.report { padding: 8px 0; border-bottom: 1px solid var(--line); }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
