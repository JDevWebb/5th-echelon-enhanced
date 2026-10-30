<template>
  <div class="page chat-page">
    <div class="inner">
      <p class="kicker">Community network</p>
      <h1 class="title">Chat</h1>
      <p class="soft intro">Talk with everyone on the community network, the players in your lobby, and your friends on any of its servers.</p>

      <p v-if="!view" class="muted working"><i class="spin"></i>Opening chat…</p>
      <p v-else-if="view.state === 'no-account'" class="muted">Set up an account on a community server first (Play).</p>
      <p v-else-if="view.state === 'opening'" class="muted working"><i class="spin"></i>Opening chat…</p>
      <p v-else-if="view.state !== 'open'" class="muted">{{ view.why }}</p>
      <template v-else>
        <p v-if="view.trouble" class="warn small">Chat is reconnecting: {{ view.trouble }}</p>
        <p v-if="view.read_only" class="warn small">The network can't be reached right now: you can read, not send.</p>
        <div class="layout" :class="{ wide: tab === 'global' }">
          <nav class="rooms" aria-label="Rooms">
            <button type="button" :class="{ on: tab === 'global' }" @click="open('global')"><b># Global</b><small>Everyone, every server</small></button>
            <button type="button" :class="{ on: tab === 'lobby' }" @click="open('lobby')"><b># Lobby</b><small>{{ view.lobby ? 'Your party or match' : 'Join a party or match first' }}</small></button>
            <p class="caps">Messages</p>
            <p v-if="!view.conversations.length" class="muted small pad">No conversations yet. Make friends in the game (or from Global) to message them.</p>
            <button v-for="r in view.conversations" :key="r.with" type="button" :class="{ on: tab === 'messages' && with_ === r.with }" @click="open('messages', r.with)">
              <b>{{ r.name }}</b><small>{{ r.what }}</small><span v-if="r.unread" class="count">{{ r.unread > 99 ? '99+' : r.unread }}</span>
            </button>
          </nav>

          <section class="room">
            <p class="room-head">
              <b>{{ tab === 'global' ? '# Global' : tab === 'lobby' ? '# Lobby' : view.with_name ?? '' }}</b>
              <small class="muted">{{ tab === 'global' ? 'The whole community network' : tab === 'lobby' ? 'The players in your party or match' : with_ ? 'Direct messages' : '' }}</small>
            </p>
            <p v-if="tab === 'messages' && !with_" class="muted">Pick a conversation, or a friend to start one.</p>
            <p v-else-if="tab === 'lobby' && !view.lobby" class="muted">Join a lobby or a match in the game: its players can talk here, and in the game's overlay.</p>
            <template v-else>
              <div ref="list" class="messages">
                <p v-if="!view.messages.length" class="muted">No messages yet.</p>
                <div v-for="m in view.messages" :key="m.id" class="msg" :class="{ me: m.mentions_me }" @contextmenu.prevent="menu = { m, x: $event.clientX, y: $event.clientY }">
                  <small class="muted">{{ m.time }}</small>
                  <b :class="{ mine: m.mine }" :title="m.server ? `On ${m.server}` : null" @click="menu = { m, x: $event.clientX, y: $event.clientY }">{{ m.mine ? 'You' : m.from_name }}</b>
                  <span class="body"><template v-for="(p, i) in m.parts" :key="i"><a v-if="p.link" href="#" title="A Discord invite: opens in your browser after you confirm" @click.prevent="invite = p.text">{{ p.text }}</a><template v-else>{{ p.text }}</template></template></span>
                  <div v-if="reporting?.id === m.id" class="report" @click.stop>
                    <b>Report {{ m.from_name }}'s message to the admins</b>
                    <input v-model="reporting.why" class="field" type="text" placeholder="Why (optional)" maxlength="300" autocomplete="off" />
                    <span class="row"><button type="button" class="btn primary small" @click="report">Report</button><button type="button" class="btn small" @click="reporting = null">Cancel</button></span>
                  </div>
                </div>
              </div>
              <p v-if="tab === 'global' && !view.may_post_global" class="muted small">{{ view.global_wait }}</p>
              <div class="composer">
                <input
                  ref="box"
                  v-model="text"
                  class="field"
                  type="text"
                  :maxlength="view.max_message"
                  :placeholder="tab === 'global' ? 'Say something to everyone…' : tab === 'lobby' ? 'Say something to your lobby…' : `Message ${view.with_name}…`"
                  :disabled="!allowed"
                  autocomplete="off"
                  @input="refused = null"
                  @keydown.enter="send"
                />
                <button type="button" class="btn primary" :disabled="!allowed || !!checked.why || !text.trim() || sending" @click="send">Send</button>
                <i v-if="sending" class="spin"></i>
              </div>
              <p v-if="refused || checked.why" class="warn small">{{ refused || checked.why }}</p>
              <p v-else-if="checked.masked" class="muted small">Some words will show as ***.</p>
            </template>
          </section>

          <aside v-if="tab === 'global'" class="online">
            <p class="caps">Online here · {{ view.online.length }}</p>
            <p v-if="!view.online.length" class="muted">Nobody else is online here right now.</p>
            <p v-for="p in view.online" :key="p.id" class="person" :title="p.activity || (p.friend ? 'Online · your friend' : 'Online')" @contextmenu.prevent="!p.friend && (who = { p, x: $event.clientX, y: $event.clientY })">
              <i :class="{ friend: p.friend }"></i>{{ p.name }}
            </p>
            <p v-if="view.online.length" class="muted small">Right-click a name for more.</p>
          </aside>
        </div>
        <p class="muted small rules">{{ view.rules }}</p>
      </template>
    </div>

    <!-- A message's menu: what can be done about it and its sender. -->
    <div v-if="menu" class="ctx" :style="{ left: `${menu.x}px`, top: `${menu.y}px` }" @click.stop>
      <template v-if="menu.m.mine"><button type="button" @click="act('delete')">Delete</button></template>
      <template v-else>
        <button v-if="menu.m.friend && tab !== 'messages'" type="button" @click="act('message')">Message {{ menu.m.from_name }}</button>
        <button v-if="!menu.m.friend" type="button" @click="act('request')">Add friend</button>
        <button type="button" @click="act('block')">Block {{ menu.m.from_name }}</button>
        <button type="button" @click="act('report')">Report…</button>
      </template>
    </div>
    <div v-if="who" class="ctx" :style="{ left: `${who.x}px`, top: `${who.y}px` }" @click.stop>
      <button v-if="!who.p.asked" type="button" @click="person('request')">Add friend</button>
      <button type="button" @click="person('invite')">Invite to your game</button>
      <button type="button" @click="person('block')">Block {{ who.p.name }}</button>
    </div>

    <Transition name="modal">
      <div v-if="invite" class="backdrop" @click.self="invite = null">
        <div v-modal="() => (invite = null)" class="dialog" role="dialog" aria-modal="true" aria-labelledby="invite-title">
          <h2 id="invite-title">Open a Discord invite?</h2>
          <p><b>{{ invite }}</b></p>
          <p class="soft">It opens in your browser. Someone in chat sent it: only join Discord servers you trust.</p>
          <div class="row end">
            <button type="button" class="btn" @click="invite = null">Cancel</button>
            <button type="button" class="btn primary" @click="openInvite">Open</button>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup>
// Chat (the egui launcher's chat.rs): Global with who's online here beside it, the lobby, and
// conversations with friends; right-click (or click a name) for what can be done about a message.
// The rules are checked while typing; the server decides, and masks slurs and hate.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { call, listen } from '../lib/tauri.js';

const view = ref(null);
const tab = ref('global');
const with_ = ref(null);
const text = ref('');
const checked = ref({ why: null, masked: false });
const refused = ref(null);
const sending = ref(false);
const menu = ref(null);
const who = ref(null);
const reporting = ref(null);
const invite = ref(null);
const list = ref(null);
const box = ref(null);
let stop = null;
let timer = null;

const allowed = computed(() => !view.value?.read_only && (tab.value !== 'global' || view.value?.may_post_global));

async function load(showing = true) {
  const atBottom = !list.value || list.value.scrollHeight - list.value.scrollTop - list.value.clientHeight < 40;
  const before = view.value?.messages.length ?? 0;
  view.value = await call('chat', { tab: tab.value, with: with_.value, showing });
  // Stuck to the newest, unless the player scrolled up to read.
  if (atBottom || before === 0) nextTick(() => list.value && (list.value.scrollTop = list.value.scrollHeight));
}

function open(t, w = null) {
  tab.value = t;
  with_.value = w;
  text.value = '';
  refused.value = null;
  load();
}

watch(text, async (t) => (checked.value = await call('chat_check', { text: t })));

async function send() {
  if (!allowed.value || checked.value.why || !text.value.trim() || sending.value) return;
  sending.value = true;
  try {
    await call('chat_send', { room: tab.value, to: with_.value ?? '', text: text.value });
    text.value = '';
    await load();
  } catch (e) {
    refused.value = String(e);
  } finally {
    sending.value = false;
    nextTick(() => box.value?.focus());
  }
}

async function act(what) {
  const m = menu.value.m;
  menu.value = null;
  if (what === 'delete') await call('chat_delete', { id: m.id });
  else if (what === 'message') open('messages', m.from);
  else if (what === 'report') reporting.value = { id: m.id, why: '' };
  // By their identity: a name alone may be another account's on this server.
  else await call('chat_act', { action: what, id: '', identity: m.from, name: m.from_name });
}

async function person(what) {
  const p = who.value.p;
  who.value = null;
  await call('chat_act', { action: what, id: p.id, identity: '', name: p.name });
}

async function report() {
  const r = reporting.value;
  reporting.value = null;
  await call('chat_report', { id: r.id, why: r.why });
}

async function openInvite() {
  const link = invite.value.startsWith('http') ? invite.value : `https://${invite.value}`;
  invite.value = null;
  await call('open_url', { url: link });
}

const close = () => {
  menu.value = null;
  who.value = null;
};
// Escape closes a menu, the report form or the invite dialog.
const escape = (e) => {
  if (e.key !== 'Escape') return;
  close();
  reporting.value = null;
  invite.value = null;
};
onMounted(async () => {
  document.addEventListener('click', close);
  document.addEventListener('keydown', escape);
  stop = await listen('chat', () => load());
  timer = setInterval(() => load(), 5000);
  load();
});
onBeforeUnmount(() => {
  document.removeEventListener('click', close);
  document.removeEventListener('keydown', escape);
  stop?.();
  clearInterval(timer);
  call('chat', { tab: tab.value, with: with_.value, showing: false });
});
</script>

<style scoped>
.intro { margin: 0 0 12px; }
.small { font-size: 12.5px; }
.warn { color: var(--warn); margin: 0 0 6px; }
.layout { display: grid; grid-template-columns: 250px 1fr; gap: 16px; align-items: start; }
.layout.wide { grid-template-columns: 250px 1fr 240px; }
@media (max-width: 1000px) { .layout.wide { grid-template-columns: 250px 1fr; } .online { display: none; } }
.rooms { display: flex; flex-direction: column; gap: 4px; max-height: calc(100vh - 290px); overflow: auto; }
.rooms button { position: relative; display: flex; flex-direction: column; align-items: flex-start; min-height: 50px; padding: 8px 14px; border-radius: 4px; text-align: left; color: var(--soft); transition: background .15s; }
.rooms button b { font-weight: 600; font-size: 15px; }
.rooms button small { font-size: 12.5px; color: var(--muted); }
.rooms button:hover { background: var(--control); }
.rooms button.on { background: var(--selected); color: var(--fg); box-shadow: inset 3px 0 0 var(--accent); }
.rooms .caps { margin: 14px 14px 2px; }
.pad { padding: 0 14px; }
.count { position: absolute; right: 12px; top: 50%; transform: translateY(-50%); min-width: 24px; padding: 0 4px; border-radius: 3px; background: var(--accent); color: var(--on-accent); font: 11px/18px PlexMono, monospace; text-align: center; }
.room-head { display: flex; align-items: baseline; gap: 10px; margin: 0 0 4px; }
.room-head b { font-weight: 600; font-size: 17px; }
.messages { height: calc(100vh - 380px); min-height: 220px; overflow: auto; padding: 10px 12px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.msg { padding: 2px 4px; border-radius: 6px; line-height: 1.5; }
.msg.me { background: rgba(155, 224, 90, .10); }
.msg small { margin-right: 6px; }
.msg > b { margin-right: 6px; font-weight: 600; cursor: pointer; }
.msg > b.mine { color: var(--accent); }
.body { user-select: text; overflow-wrap: anywhere; }
.body a { text-decoration: underline; text-underline-offset: 2px; }
.report .field { flex: none; }
.report { margin: 6px 0; padding: 10px 12px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); display: flex; flex-direction: column; gap: 8px; }
.composer { display: flex; gap: 8px; align-items: center; margin-top: 8px; }
.field { flex: 1; height: 38px; padding: 0 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; }
.field:focus { border-color: var(--accent); }
.field:disabled { opacity: .5; }
.btn:disabled { opacity: .45; cursor: default; }
.btn.small { height: 30px; padding: 0 12px; font-size: 13px; }
.row { display: flex; gap: 8px; }
.row.end { justify-content: flex-end; margin-top: 12px; }
.online .caps { margin: 0 0 8px; }
.person { display: flex; align-items: center; gap: 8px; margin: 0 0 4px; cursor: default; }
.person i { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.person i.friend { background: var(--accent); }
.rules { margin-top: 8px; }
.ctx { position: fixed; z-index: 60; min-width: 180px; padding: 4px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 12px 30px rgba(0, 0, 0, .5); display: flex; flex-direction: column; }
.ctx button { padding: 8px 12px; border-radius: 4px; text-align: left; }
.ctx button:hover { background: var(--control); }
.backdrop { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(460px, 100%); padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.dialog h2 { margin: 0 0 10px; font: 700 22px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
.dialog p { margin: 0 0 8px; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
</style>
