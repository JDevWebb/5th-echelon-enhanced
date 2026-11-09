<template>
  <section class="panel conversation">
    <!-- No conversation yet: an admin writes first (from the player's page). -->
    <template v-if="!thread && fresh">
      <header>
        <div>
          <h2>{{ fresh }} <span class="muted small mono" :title="identity">{{ identity.slice(0, 4) }}-{{ identity.slice(-4) }}</span></h2>
          <p class="small muted">No conversation yet. What you write starts one: they read it on their launcher's Support page (0.4.3 and later), and the game's overlay tells them.</p>
        </div>
      </header>
    </template>
    <div v-else-if="!thread" class="empty">{{ threadError || 'Loading…' }}</div>
    <template v-else>
      <header>
        <div>
          <h2>{{ thread.name }} <span class="muted small mono" :title="thread.identity">{{ thread.short }}</span></h2>
          <p class="small muted">
            {{ thread.server || 'no server named' }}<template v-if="thread.launcher"> · launcher {{ thread.launcher }}</template>
            <template v-for="a in thread.accounts" :key="a.server + a.id"> · <RouterLink :to="`/players/${a.server}/${a.id}`">{{ a.name }} on {{ names.get(a.server) || a.server }}</RouterLink></template>
          </p>
        </div>
        <div class="thread-actions">
          <div class="seg" role="group" aria-label="Status">
            <button v-for="s in ['open', 'waiting', 'resolved']" :key="s" type="button" :aria-pressed="String(thread.status === s)" :disabled="busy" @click="setStatus(s)">{{ STATUS_LABEL[s] }}</button>
          </div>
          <button class="small danger" type="button" :disabled="busy" @click="remove">Delete</button>
        </div>
      </header>
      <div ref="scroller" class="messages">
        <div v-for="m in thread.messages" :key="m.id" class="msg" :class="m.from">
          <div class="meta small">
            <b>{{ m.from === 'admin' ? m.admin : thread.name }}</b>
            <span class="muted" :title="fmt.when(m.at)">{{ fmt.ago(m.at) }}</span>
          </div>
          <p class="text">{{ m.text }}</p>
          <div v-if="m.files.length" class="files">
            <a v-for="f in m.files" :key="f.name" class="btn small" :href="fileUrl(m.id, f.name)" :download="`support-${m.id}-${f.name}`">{{ f.name }} <span class="muted">{{ fmt.bytes(f.size) }}</span></a>
          </div>
        </div>
      </div>
    </template>
    <form v-if="thread || fresh" class="answer" @submit.prevent="answer">
      <label class="field">
        <span>{{ thread ? 'Reply' : 'Message' }} — they read it in the launcher, and the overlay tells them ({{ text.length }}/2000)</span>
        <textarea v-model="text" maxlength="2000" rows="4" placeholder="Write a reply…"></textarea>
      </label>
      <div class="row">
        <button class="primary" type="submit" :disabled="busy || !text.trim()">Send reply</button>
        <span class="small muted">Signed with your name, {{ me }}. Sending marks it waiting on the player.</span>
      </div>
    </form>
  </section>
</template>

<script setup>
// One support conversation: its messages, its status and a reply. The Support page and the
// Inbox both show it; `changed` says the list around it should load again, `gone` that it was
// deleted.
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { api } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ identity: String, freshName: { type: String, default: '' } });
const emit = defineEmits(['changed', 'gone']);
const STATUS_LABEL = { open: 'Open', waiting: 'Waiting on player', resolved: 'Resolved' };

const names = computed(() => new Map((live.overview?.servers || []).map(sv => [sv.id, serverName(sv)])));
const me = ref('');
onMounted(async () => { try { me.value = (await api('GET', '/me')).username || ''; } catch { /* named on send */ } });

const thread = ref(null);
const threadError = ref('');
// Starting a conversation: no thread yet, and the player's name from their page's link.
const fresh = ref('');
const text = ref('');
const busy = ref(false);
const scroller = ref(null);
async function load() {
  if (!props.identity) { thread.value = null; return; }
  try {
    thread.value = await api('GET', `/support/${props.identity}`);
    threadError.value = '';
    fresh.value = '';
    await nextTick();
    if (scroller.value) scroller.value.scrollTop = scroller.value.scrollHeight;
  } catch (e) {
    if (e.status === 404 && props.freshName) fresh.value = props.freshName;
    else threadError.value = e.message;
  }
}
watch(() => props.identity, () => { thread.value = null; text.value = ''; load(); }, { immediate: true });
watch(() => live.supportTick, load);

const fileUrl = (message, name) => `/api/support/${props.identity}/files/${message}/${encodeURIComponent(name)}`;

async function answer() {
  if (!text.value.trim()) return;
  busy.value = true;
  try {
    thread.value = await api('POST', `/support/${props.identity}`, { text: text.value });
    text.value = '';
    toast('Sent. They\'ll see it in the launcher.');
    emit('changed');
    await nextTick();
    if (scroller.value) scroller.value.scrollTop = scroller.value.scrollHeight;
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
async function remove() {
  const ok = await confirmBox('Delete this conversation?', 'Every message both ways and the files the player sent are deleted for good. They can write again.', 'Delete', true);
  if (!ok) return;
  busy.value = true;
  try {
    await api('DELETE', `/support/${props.identity}`);
    toast('Deleted.');
    emit('gone');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
async function setStatus(status) {
  busy.value = true;
  try {
    await api('PUT', `/support/${props.identity}/status`, { status });
    thread.value = { ...thread.value, status };
    emit('changed');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped>
.conversation header { align-items: flex-start; gap: 12px; flex-wrap: wrap; }
.thread-actions { display: flex; align-items: center; gap: 8px; }
.messages { display: flex; flex-direction: column; gap: 12px; max-height: 60vh; overflow-y: auto; padding: 4px 2px 14px; }
.msg { max-width: 82%; padding: 10px 14px; border-radius: 10px; border: 1px solid var(--line); background: var(--panel-2); }
.msg.admin { align-self: flex-end; border-color: var(--accent); background: var(--accent-soft); }
.meta { display: flex; gap: 8px; margin-bottom: 4px; }
.text { white-space: pre-wrap; word-break: break-word; }
.files { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
.answer { display: flex; flex-direction: column; gap: 8px; border-top: 1px solid var(--line); padding-top: 12px; }
.answer textarea { font-family: var(--sans); font-size: 14px; }
</style>
