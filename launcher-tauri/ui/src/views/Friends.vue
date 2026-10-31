<template>
  <div class="page" @click="menu = null">
    <div class="inner">
      <p class="kicker">Squad</p>
      <h1 class="title">Friends</h1>

      <p v-if="!info.game_dir || !info.server" class="muted">Set up the game and a server first (Play).</p>
      <p v-else-if="people && !people.signed_in" class="muted">Make an account on your server first (Play): friends are kept by your account.</p>
      <template v-else>
        <div class="tabs" role="tablist" aria-label="Friends" @keydown="tabKeys">
          <button
            v-for="t in tabs"
            :id="`tab-${t.id}`"
            :key="t.id"
            :ref="(el) => (tabEls[t.id] = el)"
            type="button"
            role="tab"
            :aria-selected="tab === t.id"
            :aria-controls="`panel-${t.id}`"
            :tabindex="tab === t.id ? 0 : -1"
            :class="{ on: tab === t.id }"
            @click="tab = t.id"
          >
            {{ t.label }}<span v-if="t.count" class="count" :class="{ hot: t.hot }">{{ t.count }}</span>
          </button>
        </div>

        <p v-if="people?.error" class="bad small" role="alert">Couldn't read your friends: {{ people.error }} <a href="#" @click.prevent="load">Try again</a></p>
        <p v-else-if="!people" class="muted working"><i class="spin"></i>Reading your friends…</p>

        <!-- Friends: online first, what they're doing, and what can be done. -->
        <section v-if="people && tab === 'friends'" id="panel-friends" class="panel" role="tabpanel" aria-labelledby="tab-friends">
          <p v-if="!people.friends.length" class="empty">
            No friends yet. <a href="#" @click.prevent="tab = 'find'">Find players</a> to add some, or accept a request when one comes.
          </p>
          <ul v-else class="rows">
            <li v-for="p in people.friends" :key="p.id" class="person" :class="{ off: !p.online }">
              <span class="who">
                <span class="avatar" aria-hidden="true">{{ initial(p.name) }}<i v-if="p.online"></i></span>
                <span class="names">
                  <b :title="p.name">{{ p.name }}</b>
                  <small>{{ p.online ? p.activity || 'Online' : 'Offline' }}<template v-if="p.identity"> · {{ p.identity }}</template></small>
                </span>
              </span>
              <span class="acts">
                <button v-if="p.joinable" type="button" class="btn small primary" :disabled="busy" @click="act('join', p)">Join</button>
                <button v-else-if="p.online && info.running" type="button" class="btn small" :disabled="busy" @click="act('invite', p)">Invite</button>
                <More :p="p" :menu="menu" :items="[['remove', 'Remove friend'], ['block', 'Block']]" @open="(m) => (menu = m)" @pick="(a) => ask(a, p)" />
              </span>
            </li>
          </ul>
          <template v-if="people.elsewhere.length">
            <p class="caps sub">Playing on other servers</p>
            <ul class="rows">
              <li v-for="e in people.elsewhere" :key="`${e.name}@${e.host}`" class="person">
                <span class="who">
                  <span class="avatar" aria-hidden="true">{{ initial(e.name) }}<i></i></span>
                  <span class="names">
                    <b :title="e.name">{{ e.name }}</b>
                    <small>On {{ e.server }}<template v-if="e.region"> ({{ e.region }})</template>: to play together, one of you joins the other's server (the server menu on Play)</small>
                  </span>
                </span>
              </li>
            </ul>
          </template>
        </section>

        <!-- Requests: theirs to answer, the player's to take back. -->
        <section v-if="people && tab === 'requests'" id="panel-requests" class="panel" role="tabpanel" aria-labelledby="tab-requests">
          <p v-if="!people.received.length && !people.sent.length" class="empty">No friend requests either way.</p>
          <template v-if="people.received.length">
            <p class="caps sub">Waiting for your answer</p>
            <ul class="rows">
              <li v-for="p in people.received" :key="p.id" class="person">
                <span class="who">
                  <span class="avatar" aria-hidden="true">{{ initial(p.name) }}<i v-if="p.online"></i></span>
                  <span class="names"><b :title="p.name">{{ p.name }}</b><small>Wants to be friends</small></span>
                </span>
                <span class="acts">
                  <button type="button" class="btn small primary" :disabled="busy" @click="act('accept', p)">Accept</button>
                  <button type="button" class="btn small" :disabled="busy" @click="act('decline', p)">Decline</button>
                  <More :p="p" :menu="menu" :items="[['block', 'Block']]" @open="(m) => (menu = m)" @pick="(a) => ask(a, p)" />
                </span>
              </li>
            </ul>
          </template>
          <template v-if="people.sent.length">
            <p class="caps sub">You asked</p>
            <ul class="rows">
              <li v-for="p in people.sent" :key="p.id" class="person">
                <span class="who">
                  <span class="avatar" aria-hidden="true">{{ initial(p.name) }}<i v-if="p.online"></i></span>
                  <span class="names"><b :title="p.name">{{ p.name }}</b><small>Hasn't answered yet</small></span>
                </span>
                <span class="acts">
                  <button type="button" class="btn small" :disabled="busy" @click="act('decline', p)">Cancel request</button>
                </span>
              </li>
            </ul>
          </template>
        </section>

        <!-- Find players: by name; who's online before anything is typed. -->
        <section v-if="people && tab === 'find'" id="panel-find" class="panel" role="tabpanel" aria-labelledby="tab-find">
          <form class="search" @submit.prevent="search">
            <input v-model="query" class="field" type="search" maxlength="32" placeholder="A player's name" aria-label="A player's name" autocomplete="off" spellcheck="false" />
            <button type="submit" class="btn" :disabled="searching">Search</button>
          </form>
          <p v-if="searchError" class="bad small" role="alert">{{ searchError }}</p>
          <p v-else-if="searching" class="muted working"><i class="spin"></i>Looking…</p>
          <p v-else-if="found && !found.length" class="empty">{{ searched ? `Nobody called “${searched}” here.` : 'Nobody else is online right now.' }}</p>
          <template v-else-if="found">
            <p class="caps sub">{{ searched ? `Players called “${searched}”` : 'Online now' }}</p>
            <ul class="rows">
              <li v-for="p in found" :key="p.id" class="person">
                <span class="who">
                  <span class="avatar" aria-hidden="true">{{ initial(p.name) }}<i v-if="p.online"></i></span>
                  <span class="names">
                    <b :title="p.name">{{ p.name }}</b>
                    <small>{{ p.online ? p.activity || 'Online' : 'Offline' }}<template v-if="p.identity"> · {{ p.identity }}</template></small>
                  </span>
                </span>
                <span class="acts">
                  <span v-if="p.relation === 'friend'" class="muted small">Your friend</span>
                  <span v-else-if="p.relation === 'sent'" class="muted small">Asked</span>
                  <button v-else-if="p.relation === 'received'" type="button" class="btn small primary" :disabled="busy" @click="act('accept', p)">Accept</button>
                  <button v-else type="button" class="btn small" :disabled="busy" @click="act('request', p)">Add friend</button>
                  <More :p="p" :menu="menu" :items="[['block', 'Block']]" @open="(m) => (menu = m)" @pick="(a) => ask(a, p)" />
                </span>
              </li>
            </ul>
          </template>
        </section>

        <!-- Blocked: who, and the way back. -->
        <section v-if="people && tab === 'blocked'" id="panel-blocked" class="panel" role="tabpanel" aria-labelledby="tab-blocked">
          <p class="soft small explain">{{ BLOCK_MEANS }} A block of a player with a 5th Echelon identity holds on every server that shares friends, when you have one too.</p>
          <p v-if="!people.blocked.length" class="empty">You haven't blocked anyone.</p>
          <ul v-else class="rows">
            <li v-for="p in people.blocked" :key="p.id" class="person">
              <span class="who">
                <span class="avatar" aria-hidden="true">{{ initial(p.name) }}</span>
                <span class="names"><b :title="p.name">{{ p.name }}</b><small v-if="p.identity">{{ p.identity }}</small></span>
              </span>
              <span class="acts">
                <button type="button" class="btn small" :disabled="busy" @click="act('unblock', p)">Unblock</button>
              </span>
            </li>
          </ul>
        </section>

        <p v-if="undo" class="undo" role="status">
          {{ undo.line }} <button type="button" class="link" @click="act(undo.back, undo.p); undo = null">Undo</button>
        </p>
      </template>
    </div>

    <!-- Remove and Block ask once, saying what they do. -->
    <Transition name="modal">
      <div v-if="asking" class="backdrop" @click.self="asking = null">
        <div v-modal class="dialog" role="dialog" aria-modal="true" aria-labelledby="ask-title" @keydown.esc="asking = null">
          <h2 id="ask-title">{{ asking.action === 'block' ? `Block ${asking.p.name}?` : `Remove ${asking.p.name} from your friends?` }}</h2>
          <p class="soft">{{ asking.action === 'block' ? BLOCK_MEANS : 'You can ask them to be friends again later.' }}</p>
          <div class="row end">
            <button type="button" class="btn" @click="asking = null">Cancel</button>
            <button type="button" class="btn danger" @click="confirm">{{ asking.action === 'block' ? 'Block' : 'Remove' }}</button>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup>
// Friends (the 0.4.8 plan's P1): one place for friends, requests both ways, finding players
// and who's blocked, with the same words as the overlay. Remove and Block ask first; Block can
// be undone at once.
import { computed, defineComponent, h, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { call, listen } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true } });
const emit = defineEmits(['go', 'changed']);

const BLOCK_MEANS =
  "Neither of you can message, invite or friend the other, you don't see each other online, and quick match leaves out matches they're in.";

const people = ref(null);
const tab = ref('friends');
const busy = ref(false);
const menu = ref(null);
const asking = ref(null);
const undo = ref(null);
let undoTimer = null;

async function load() {
  people.value = await call('people');
}

const tabs = computed(() => {
  const p = people.value;
  const online = p?.friends.filter((f) => f.online).length ?? 0;
  return [
    { id: 'friends', label: 'Friends', count: p?.friends.length ? `${online}/${p.friends.length}` : null },
    { id: 'requests', label: 'Requests', count: p ? p.received.length + p.sent.length || null : null, hot: (p?.received.length ?? 0) > 0 },
    { id: 'find', label: 'Find players', count: null },
    { id: 'blocked', label: 'Blocked', count: p?.blocked.length || null },
  ];
});
const tabEls = {};
function tabKeys(e) {
  const ids = tabs.value.map((t) => t.id);
  const at = ids.indexOf(tab.value);
  const to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: ids.length - 1 }[e.key];
  if (to === undefined) return;
  e.preventDefault();
  tab.value = ids[(to + ids.length) % ids.length];
  nextTick(() => tabEls[tab.value]?.focus());
}

const initial = (name) => (name || '?').trim().charAt(0).toUpperCase();

/** Asks first for what can't be taken back lightly. */
function ask(action, p) {
  menu.value = null;
  asking.value = { action, p };
}
function confirm() {
  const { action, p } = asking.value;
  asking.value = null;
  act(action, p);
}

async function act(action, p) {
  busy.value = true;
  try {
    if (action === 'join') await call('join_friend', { id: p.id, name: p.name });
    else await call('player_action', { action, id: p.id, name: p.name });
  } finally {
    busy.value = false;
  }
  clearTimeout(undoTimer);
  undo.value = action === 'block' ? { line: `Blocked ${p.name}.`, back: 'unblock', p } : null;
  if (undo.value) undoTimer = setTimeout(() => (undo.value = null), 12000);
  await load();
  if (found.value) search();
  emit('changed');
}

// Find players: who's online at first, then by name.
const query = ref('');
const searched = ref('');
const found = ref(null);
const searching = ref(false);
const searchError = ref(null);
async function search() {
  searching.value = true;
  searchError.value = null;
  try {
    found.value = await call('find_players', { query: query.value.trim() });
    searched.value = query.value.trim();
  } catch (e) {
    searchError.value = String(e);
  } finally {
    searching.value = false;
  }
}

// A row's "⋯" menu: its button, and the menu under it (Escape closes it back to the button).
const More = defineComponent({
  props: { p: Object, menu: Object, items: Array },
  emits: ['open', 'pick'],
  setup(mp, { emit: memit }) {
    const button = ref(null);
    return () => {
      const open = mp.menu?.id === mp.p.id;
      const close = () => {
        memit('open', null);
        button.value?.focus();
      };
      return h('span', { class: 'more' }, [
        h(
          'button',
          {
            ref: button,
            type: 'button',
            class: 'btn small dots',
            'aria-label': `More for ${mp.p.name}`,
            'aria-haspopup': 'menu',
            'aria-expanded': open,
            onClick: (e) => {
              e.stopPropagation();
              memit('open', open ? null : { id: mp.p.id });
              if (!open) nextTick(() => button.value?.nextElementSibling?.querySelector('button')?.focus());
            },
          },
          '⋯',
        ),
        open &&
          h(
            'span',
            {
              class: 'menu',
              role: 'menu',
              onClick: (e) => e.stopPropagation(),
              onKeydown: (e) => {
                const items = [...e.currentTarget.querySelectorAll('button')];
                const at = items.indexOf(document.activeElement);
                if (e.key === 'Escape') close();
                else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                  e.preventDefault();
                  items[(at + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
                }
              },
            },
            mp.items.map(([action, label]) =>
              h('button', { type: 'button', role: 'menuitem', class: action === 'block' ? 'danger' : null, onClick: () => memit('pick', action) }, label),
            ),
          ),
      ]);
    };
  },
});

let stop = null;
let timer = null;
onMounted(async () => {
  load();
  search();
  timer = setInterval(load, 20000);
  stop = await listen('checks', load);
});
onBeforeUnmount(() => {
  clearInterval(timer);
  clearTimeout(undoTimer);
  stop?.();
});
</script>

<style scoped>
.tabs { display: flex; gap: 4px; margin: 4px 0 16px; border-bottom: 1px solid var(--line); }
.tabs button { position: relative; padding: 10px 14px; color: var(--muted); font-weight: 600; font-size: 14px; display: flex; align-items: center; gap: 8px; }
.tabs button:hover { color: var(--fg); }
.tabs button.on { color: var(--fg); }
.tabs button.on::after { content: ""; position: absolute; left: 10px; right: 10px; bottom: -1px; height: 2px; background: var(--accent); }
.count { font: 11px PlexMono, monospace; padding: 1px 6px; border-radius: 9px; background: var(--control); color: var(--soft); }
.count.hot { background: var(--accent); color: var(--on-accent); }
.panel { max-width: 760px; }
.rows { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
.person { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 8px 10px 8px 12px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.person.off .names b { color: var(--soft); }
.who { flex: 1; min-width: 0; display: flex; align-items: center; gap: 12px; }
.avatar { position: relative; flex: none; width: 34px; height: 34px; border-radius: 50%; display: grid; place-items: center; font-weight: 600; color: var(--accent); background: var(--selected); border: 1.5px solid rgba(155, 224, 90, .4); }
.person.off .avatar { color: var(--muted); border-color: var(--control-line); background: var(--control); }
.avatar i { position: absolute; right: -1px; bottom: -1px; width: 10px; height: 10px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 2px var(--sunken); }
.names { min-width: 0; display: flex; flex-direction: column; gap: 1px; }
.names b { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.names small { font-size: 12.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
.acts { flex: none; display: flex; align-items: center; gap: 8px; }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.btn:disabled { opacity: .45; cursor: default; }
.btn.danger { background: var(--bad); border-color: var(--bad); color: var(--on-accent); }
:deep(.more) { position: relative; display: inline-flex; }
:deep(.dots) { width: 34px; padding: 0; font-size: 16px; letter-spacing: .05em; }
:deep(.menu) { position: absolute; right: 0; top: calc(100% + 4px); min-width: 160px; padding: 4px; display: flex; flex-direction: column; border-radius: 6px; background: rgba(12, 17, 18, .97); border: 1px solid var(--control-line); box-shadow: 0 12px 30px rgba(0, 0, 0, .5); z-index: 5; }
:deep(.menu button) { padding: 8px 10px; border-radius: 4px; text-align: left; font-size: 13.5px; }
:deep(.menu button:hover), :deep(.menu button:focus-visible) { background: var(--control); outline: none; }
:deep(.menu button.danger) { color: var(--bad); }
.caps.sub { margin: 18px 0 8px; }
.caps.sub:first-child { margin-top: 0; }
.empty { color: var(--soft); margin: 6px 0; }
.empty a, .bad a { text-decoration: underline; }
.explain { margin: 0 0 12px; max-width: 640px; }
.search { display: flex; gap: 10px; margin-bottom: 14px; max-width: 520px; }
.field { flex: 1; height: 38px; padding: 0 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; }
.field:focus { border-color: var(--accent); }
.small { font-size: 12.5px; }
.bad { color: var(--bad); }
.undo { position: sticky; bottom: 0; margin: 16px 0 0; padding: 10px 14px; max-width: 760px; border-radius: 6px; background: var(--control); border: 1px solid var(--control-line); }
.link { color: var(--accent); font-weight: 600; margin-left: 6px; }
.link:hover { text-decoration: underline; }
.backdrop { position: fixed; inset: 0; z-index: 60; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(480px, 100%); padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.dialog h2 { margin: 0 0 10px; font: 700 22px/1.1 PlexCond, sans-serif; text-transform: uppercase; overflow-wrap: anywhere; }
.dialog p { margin: 0 0 10px; }
.row { display: flex; gap: 10px; }
.row.end { justify-content: flex-end; margin-top: 8px; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 16px; height: 16px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
