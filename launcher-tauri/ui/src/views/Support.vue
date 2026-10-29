<template>
  <div ref="page" class="page">
    <div class="inner">
      <p class="kicker">The network's admins</p>
      <h1 class="title">Support</h1>
      <p class="soft intro">Write to the community network's admins about anything that goes wrong. They answer here, and you're told when they do, here and in the game.</p>
      <div class="cols">
        <div class="main">
          <p v-if="view && !view.identity" class="soft">Connect to a server first: your messages are signed with the identity the launcher makes then.</p>
          <template v-else-if="view">
            <p v-if="view.error" class="muted">{{ view.error }}</p>
            <p v-if="!view.messages" class="muted working"><i class="spin"></i>Reading your conversation…</p>
            <p v-else-if="!view.messages.length" class="muted">Nothing yet. Say what went wrong: when, on which server, and what you saw. Ticking the logs below helps the admins find it.</p>
            <TransitionGroup name="msg" tag="div" class="thread">
              <div v-for="(m, i) in view.messages ?? []" :key="i" class="msg" :class="{ admin: m.admin }">
                <p class="who"><b>{{ m.who }}</b><small class="muted">{{ m.when }}</small></p>
                <p class="text">{{ m.text }}</p>
                <p v-if="m.files" class="muted small">{{ m.files }}</p>
              </div>
            </TransitionGroup>
            <p v-if="view.resolved" class="muted small">The admins marked this solved. Write again if it isn't, and it opens again.</p>
            <section class="card form">
              <p class="row-between"><span class="caps">Message</span><span class="muted small">{{ text.length }}/{{ view.max_text }}</span></p>
              <textarea v-model="text" rows="4" :maxlength="view.max_text" placeholder="What went wrong, when, and on which server?"></textarea>
              <label class="tick" :class="{ off: !view.has_game }"><input v-model="gameLog" type="checkbox" :disabled="!view.has_game" />The game's log (its last part, and the one before)</label>
              <label class="tick"><input v-model="launcherLog" type="checkbox" />The launcher's log</label>
              <p class="muted small">Your PC's name, your user folder and your internet address are hidden in them first.</p>
              <div class="send">
                <button type="button" class="btn primary" :disabled="!!why || view.sending" @click="send">Send</button>
                <i v-if="view.sending" class="spin"></i>
                <span v-else-if="why && text.trim()" class="muted">{{ why }}</span>
              </div>
            </section>
          </template>
        </div>
        <aside>
          <section class="card">
            <p class="caps">Something went wrong?</p>
            <button type="button" class="btn wide" title="The report goes to your server's admins, with the game's and the launcher's logs" @click="call('ask_feedback')">Send a report with the logs</button>
            <button type="button" class="btn wide" title="The Play screen's Status" @click="$emit('go', 'play')">Run all checks</button>
          </section>
          <section class="card">
            <p class="caps">Field manual</p>
            <a v-for="[name, url] in view?.manual ?? []" :key="url" href="#" @click.prevent="call('open_url', { url })">{{ name }}</a>
          </section>
        </aside>
      </div>
    </div>
  </div>
</template>

<script setup>
// Support (the egui launcher's support.rs): the conversation with the community network's
// admins, the newest at the foot, a message with the logs ticked, and beside it a report, the
// checks and the help pages.
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { call, listen } from '../lib/tauri.js';

defineEmits(['go']);

const view = ref(null);
const text = ref('');
// The logs help most; the player sees what they are and can untick them.
const gameLog = ref(true);
const launcherLog = ref(true);
const why = ref(null);
const page = ref(null);
let stops = [];

async function load(open = true) {
  const v = await call('support', { open });
  // A message started elsewhere (a name's review): without the logs, which it needs none of.
  if (v.draft && !text.value.trim()) {
    text.value = v.draft;
    gameLog.value = false;
    launcherLog.value = false;
  }
  const grew = (v.messages?.length ?? 0) > (view.value?.messages?.length ?? 0);
  view.value = v;
  // Stuck to the newest, as egui's.
  if (grew) nextTick(() => page.value?.scrollTo({ top: page.value.scrollHeight, behavior: 'smooth' }));
}

watch(text, async (t) => (why.value = await call('support_check', { text: t })), { immediate: true });

async function send() {
  await call('support_send', { text: text.value, gameLog: gameLog.value, launcherLog: launcherLog.value });
  load();
}

onMounted(async () => {
  load();
  stops = [await listen('support', () => load()), await listen('support-sent', () => (text.value = ''))];
});
onBeforeUnmount(() => {
  stops.forEach((s) => s());
  call('support', { open: false });
});
</script>

<style scoped>
.intro { margin: 0 0 14px; }
.cols { display: grid; grid-template-columns: 1fr 300px; gap: 20px; align-items: start; }
@media (max-width: 900px) { .cols { grid-template-columns: 1fr; } }
.thread { display: flex; flex-direction: column; gap: 8px; margin-bottom: 8px; }
.msg { max-width: 78%; min-width: 280px; align-self: flex-end; padding: 10px 14px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.msg.admin { align-self: flex-start; background: var(--surface); border-color: rgba(155, 224, 90, .35); }
.msg p { margin: 0; }
.who { display: flex; gap: 10px; align-items: baseline; margin-bottom: 4px !important; }
.admin .who b { color: var(--accent); }
.text { white-space: pre-wrap; user-select: text; }
.small { font-size: 12.5px; }
.form { margin-top: 16px; display: flex; flex-direction: column; gap: 6px; }
.form p { margin: 0; }
.row-between { display: flex; justify-content: space-between; align-items: center; }
textarea { width: 100%; padding: 8px 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; resize: vertical; }
textarea:focus { border-color: var(--accent); }
.tick { display: flex; align-items: center; gap: 10px; cursor: pointer; }
.tick.off { opacity: .5; cursor: default; }
.tick input { accent-color: var(--accent); width: 16px; height: 16px; }
.send { display: flex; align-items: center; gap: 12px; margin-top: 6px; }
.btn:disabled { opacity: .45; cursor: default; }
aside { display: flex; flex-direction: column; gap: 14px; }
aside .card { display: flex; flex-direction: column; gap: 8px; }
aside .card p { margin: 0 0 2px; }
.btn.wide { width: 100%; }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
.msg-enter-active { transition: opacity .3s, transform .3s var(--ease); }
.msg-enter-from { opacity: 0; transform: translateY(8px); }
</style>
