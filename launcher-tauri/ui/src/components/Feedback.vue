<template>
  <Transition name="modal">
    <div v-if="ask" class="backdrop">
      <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="fb-title">
        <h2 id="fb-title">How did that go?</h2>
        <p class="muted">{{ ask.why }}</p>
        <p v-if="ask.mismatch" class="muted">{{ ask.mismatch }}</p>
        <div class="middle">
          <div class="rate" role="radiogroup" aria-label="How it went">
            <button type="button" role="radio" :aria-checked="good === true" :class="{ on: good === true }" @click="good = true">👍 Good</button>
            <button type="button" role="radio" :aria-checked="good === false" :class="{ on: good === false }" @click="good = false">👎 Not great</button>
          </div>
          <p class="label">What went wrong? (tick any)</p>
          <label v-for="[id, label] in ask.problems" :key="id" class="tick">
            <input v-model="problems" type="checkbox" :value="id" />
            <span>{{ label }}</span>
          </label>
          <p class="label">Anything else? (who you played with, what you were doing)</p>
          <textarea v-model="comment" rows="3" maxlength="2000"></textarea>
          <label class="tick">
            <input v-model="attach" type="checkbox" />
            <span>Send my logs too</span>
          </label>
          <p class="muted small">Your PC's name, your user folder and your internet address are hidden before they're sent.</p>
          <details v-if="attach && ask.files.length" class="sent">
            <summary>What's sent</summary>
            <button v-for="([name, kb], i) in ask.files" :key="name" type="button" class="file" :class="{ on: viewing === i }" @click="view(i)">{{ name }} ({{ kb }} KB)</button>
            <pre v-if="viewing != null && text != null">{{ text }}</pre>
          </details>
        </div>
        <div class="actions">
          <button type="button" class="btn primary" :disabled="!canSend" @click="send">Send</button>
          <button type="button" class="btn" @click="close(false)">Not now</button>
          <button v-if="!ask.manual" type="button" class="btn" title="Settings › Feedback asks again" @click="close(true)">Don't ask again</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup>
// "How did that go?" after a game (the egui launcher's feedback.rs): what made it ask, the
// problems ticked from what went wrong, a comment, and the logs (private parts hidden). Sending
// carries on in the activity bar.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { call, listen } from '../lib/tauri.js';

const ask = ref(null);
const good = ref(null);
const problems = ref([]);
const comment = ref('');
const attach = ref(true);
const viewing = ref(null);
const text = ref(null);

const canSend = computed(() => good.value !== null || problems.value.length > 0 || comment.value.trim() !== '');

async function refresh() {
  const a = await call('feedback');
  if (a && !ask.value) {
    good.value = null;
    problems.value = [...a.ticked];
    comment.value = '';
    attach.value = a.attach;
    viewing.value = null;
    text.value = null;
  }
  ask.value = a;
}

async function view(i) {
  viewing.value = viewing.value === i ? null : i;
  text.value = viewing.value == null ? null : await call('feedback_file', { index: i });
}

async function send() {
  await call('feedback_send', { good: good.value, problems: problems.value, comment: comment.value, attach: attach.value });
  ask.value = null;
}

async function close(never) {
  await call('feedback_close', { never });
  ask.value = null;
}

defineExpose({ refresh });
let stop = null;
onMounted(async () => {
  stop = await listen('feedback', refresh);
  refresh();
});
onBeforeUnmount(() => stop?.());
</script>

<style scoped>
.backdrop { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(600px, 100%); max-height: calc(100vh - 60px); display: flex; flex-direction: column; padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
h2 { margin: 0 0 4px; font: 700 26px/1.1 PlexCond, sans-serif; text-transform: uppercase; }
.dialog > p { margin: 0 0 4px; }
.middle { overflow: auto; min-height: 0; margin-top: 10px; padding-right: 4px; }
.rate { display: flex; gap: 8px; margin-bottom: 8px; }
.rate button { height: 36px; padding: 0 14px; border-radius: 4px; background: var(--control); border: 1px solid var(--control-line); transition: background .15s, border-color .15s; }
.rate button.on { background: var(--selected); border-color: var(--accent); color: var(--accent); }
.label { margin: 12px 0 6px; }
.tick { display: flex; align-items: center; gap: 10px; padding: 3px 0; cursor: pointer; }
.tick input { accent-color: var(--accent); width: 16px; height: 16px; }
textarea { width: 100%; padding: 8px 10px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); resize: vertical; outline: none; }
textarea:focus { border-color: var(--accent); }
.small { font-size: 12.5px; margin: 2px 0 8px; }
.sent summary { cursor: pointer; color: var(--soft); margin: 4px 0; }
.file { display: block; padding: 4px 8px; border-radius: 4px; font-size: 13px; color: var(--soft); }
.file.on, .file:hover { background: var(--control); color: var(--fg); }
pre { max-height: 220px; overflow: auto; margin: 6px 0; padding: 8px 10px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--line); font: 11px/1.45 PlexMono, monospace; color: var(--soft); white-space: pre; user-select: text; }
.actions { display: flex; gap: 10px; margin-top: 14px; }
.btn:disabled { opacity: .45; cursor: default; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
</style>
