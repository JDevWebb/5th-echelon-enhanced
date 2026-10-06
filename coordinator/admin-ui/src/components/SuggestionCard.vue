<template>
  <article class="sug" :aria-labelledby="`sug-${s.id}`">
    <div class="head">
      <div class="meta">
        <span class="pill" :class="STATUS_CLASS[s.status] || ''">{{ statusLabel(s.status) }}</span>
        <span class="pill">{{ s.area }}</span>
        <span v-if="s.item != null" class="pill ok">On the roadmap</span>
      </div>
      <span class="from" :title="fmt.when(s.created_at)">{{ fmt.ago(s.created_at) }}</span>
    </div>
    <h3 :id="`sug-${s.id}`">{{ s.title }}</h3>
    <p class="text">{{ s.text }}</p>
    <span class="from">{{ [s.name, s.server, s.launcher ? `launcher ${s.launcher}` : ''].filter(Boolean).join(' · ') }}</span>
    <form class="answer" @submit.prevent="save">
      <label class="field status"><span>Status</span>
        <select v-model="status"><option v-for="[id, label] in SUGGESTION_STATUSES" :key="id" :value="id">{{ label }}</option></select>
      </label>
      <label class="field reply"><span>Reply to the player ({{ reply.length }}/300)</span>
        <textarea v-model="reply" maxlength="300" rows="2" placeholder="Thanks! It's planned for 0.4.3."></textarea>
      </label>
      <div class="row end buttons">
        <template v-if="s.item == null">
          <select v-model="lane" class="small-select" :aria-label="`Lane to promote ${s.title} to`">
            <option v-for="[id, label] in PROMOTE_LANES" :key="id" :value="id">{{ label }}</option>
          </select>
          <button class="small" type="button" :disabled="busy" @click="promote">Promote</button>
        </template>
        <button class="primary small" type="submit" :disabled="busy || !changed">Save</button>
      </div>
    </form>
  </article>
</template>

<script setup>
// A player's suggestion in the inbox: answer it, or promote it onto the roadmap.
import { computed, ref, watch } from 'vue';
import { api } from '../lib/api.js';
import { fmt } from '../lib/fmt.js';
import { PROMOTE_LANES, SUGGESTION_STATUSES } from '../lib/roadmap.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ s: Object });
const emit = defineEmits(['changed']);
const STATUS_CLASS = { new: 'info', planned: 'warn', done: 'ok', declined: '' };
const statusLabel = id => SUGGESTION_STATUSES.find(([x]) => x === id)?.[1] || id;

const status = ref(props.s.status);
const reply = ref(props.s.reply || '');
const lane = ref('requested');
const busy = ref(false);
// A reload brings the saved answer back.
watch(() => [props.s.status, props.s.reply], ([st, re]) => { status.value = st; reply.value = re || ''; });
const changed = computed(() => status.value !== props.s.status || reply.value.trim() !== (props.s.reply || ''));

async function save() {
  busy.value = true;
  try {
    await api('PUT', `/suggestions/${props.s.id}`, { status: status.value, reply: reply.value.trim() });
    toast(reply.value.trim() ? 'Saved. The player sees your reply in their launcher.' : 'Saved.');
    emit('changed');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
async function promote() {
  busy.value = true;
  try {
    await api('POST', `/suggestions/${props.s.id}/promote`, { lane: lane.value });
    toast(`Added under ${PROMOTE_LANES.find(([id]) => id === lane.value)[1]}, hidden until you make it public.`);
    emit('changed');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped>
.sug { background: var(--panel-2); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.head { display: flex; justify-content: space-between; gap: 10px; align-items: center; flex-wrap: wrap; }
.meta { display: flex; flex-wrap: wrap; gap: 6px; }
.meta .pill { text-transform: uppercase; font-size: 10px; letter-spacing: 0.06em; }
h3 { font: 600 16px/1.25 var(--sans); letter-spacing: 0; }
.text { color: var(--soft); white-space: pre-wrap; overflow-wrap: anywhere; }
.from { font: 12px var(--mono); color: var(--faint); }
.answer { display: grid; grid-template-columns: 160px minmax(0, 1fr); gap: 10px 12px; margin-top: 8px; align-items: start; }
.answer textarea { font-family: var(--sans); font-size: 14px; min-height: 56px; }
.buttons { grid-column: 1 / -1; gap: 8px; }
.small-select { width: auto; padding: 6px 8px; font-size: 12px; }
@media (max-width: 760px) { .answer { grid-template-columns: minmax(0, 1fr); } }
</style>
