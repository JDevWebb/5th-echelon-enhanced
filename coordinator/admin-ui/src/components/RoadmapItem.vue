<template>
  <article class="item" :class="{ hidden: !item.public, editing }" :aria-labelledby="`item-${item.id}`" :title="item.updated_by ? `Changed by ${item.updated_by}, ${fmt.ago(item.updated_at)}` : null">
    <form v-if="editing" class="stack edit" @submit.prevent="save">
      <label class="field"><span>Title</span><input v-model="draft.title" required maxlength="80"></label>
      <label class="field"><span>What it is</span><textarea v-model="draft.body" maxlength="600" rows="4"></textarea></label>
      <label class="field"><span>Tags, separated by commas</span><input v-model="draft.tags" maxlength="110" placeholder="Launcher, Server"></label>
      <div class="two">
        <label class="field"><span>Status</span><input v-model="draft.status" maxlength="24" :list="`statuses-${item.id}`"></label>
        <label class="field"><span>Lane</span>
          <select v-model="draft.lane"><option v-for="l in lanes" :key="l.id" :value="l.id">{{ l.title }}</option></select>
        </label>
      </div>
      <datalist :id="`statuses-${item.id}`"><option v-for="s in STATUS_HINTS" :key="s" :value="s"></option></datalist>
      <label class="field"><span>Asked for by</span><input v-model="draft.source" maxlength="60" placeholder="Discord · Kiwi"></label>
      <label class="tick"><input v-model="draft.public" type="checkbox"> Show it on the public roadmap</label>
      <p v-if="problem" class="small warn-text" role="status">{{ problem }}</p>
      <div class="row end">
        <button class="ghost small" type="button" @click="editing = false">Cancel</button>
        <button class="primary small" type="submit" :disabled="busy || !!problem">Save</button>
      </div>
    </form>
    <template v-else>
      <div class="meta">
        <span v-if="item.status" class="pill" :class="statusClass(item.status)">{{ item.status }}</span>
        <span v-for="t in item.tags" :key="t" class="pill">{{ t }}</span>
        <span v-if="!item.public" class="pill hidden-pill" title="Only admins see it">Hidden</span>
      </div>
      <h3 :id="`item-${item.id}`">{{ item.title }}</h3>
      <p v-if="item.body">{{ item.body }}</p>
      <span v-if="item.source" class="from">{{ item.source }}</span>
      <div v-if="deleting" class="row confirm" role="group" :aria-label="`Delete ${item.title}?`">
        <span class="small">Delete it for good?</span>
        <button class="danger small" type="button" :disabled="busy" @click="remove">Delete</button>
        <button class="ghost small" type="button" @click="deleting = false">Keep</button>
      </div>
      <div v-else class="acts">
        <button class="small" type="button" @click="startEdit">Edit</button>
        <button class="small" type="button" @click="$emit('prompt', item)">Prompt</button>
        <button class="ghost small" type="button" :aria-pressed="String(item.public)" :disabled="busy" :title="item.public ? 'Shown on the public roadmap' : 'Only admins see it'" @click="togglePublic">{{ item.public ? 'Public' : 'Hidden' }}</button>
        <span class="spacer"></span>
        <button class="ghost small icon" type="button" :disabled="first || busy" :aria-label="`Move ${item.title} up`" title="Move up" @click="$emit('move', -1)">↑</button>
        <button class="ghost small icon" type="button" :disabled="last || busy" :aria-label="`Move ${item.title} down`" title="Move down" @click="$emit('move', 1)">↓</button>
        <button class="ghost small icon" type="button" :aria-label="`Delete ${item.title}`" title="Delete" @click="deleting = true">✕</button>
      </div>
    </template>
  </article>
</template>

<script setup>
// One item on the roadmap: shown, or edited in place.
import { computed, reactive, ref } from 'vue';
import { api } from '../lib/api.js';
import { fmt } from '../lib/fmt.js';
import { STATUS_HINTS, statusClass } from '../lib/roadmap.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ item: Object, lanes: Array, first: Boolean, last: Boolean, nextPosition: Object });
const emit = defineEmits(['prompt', 'move', 'changed']);

const editing = ref(false);
const deleting = ref(false);
const busy = ref(false);
const draft = reactive({ title: '', body: '', tags: '', status: '', lane: '', source: '', public: true });

function startEdit() {
  Object.assign(draft, {
    title: props.item.title, body: props.item.body || '', tags: (props.item.tags || []).join(', '),
    status: props.item.status || '', lane: props.item.lane, source: props.item.source || '', public: props.item.public,
  });
  editing.value = true;
}
const tags = computed(() => draft.tags.split(',').map(t => t.trim()).filter(Boolean));
const problem = computed(() => {
  if (!draft.title.trim()) return 'An item needs a title.';
  if (tags.value.length > 5) return 'Up to 5 tags.';
  if (tags.value.some(t => t.length > 20)) return 'Tags are up to 20 characters each.';
  return '';
});

/** The item as PUT wants it, with `changes` on top. */
const body = changes => ({
  lane: props.item.lane, title: props.item.title, body: props.item.body || '', tags: props.item.tags || [],
  status: props.item.status || '', public: props.item.public, source: props.item.source || '', position: props.item.position, ...changes,
});
async function put(changes, done) {
  busy.value = true;
  try {
    await api('PUT', `/roadmap/items/${props.item.id}`, body(changes));
    if (done) toast(done);
    emit('changed');
    return true;
  } catch (e) {
    toast(e.message, true);
    return false;
  } finally {
    busy.value = false;
  }
}
async function save() {
  if (problem.value) return;
  const moved = draft.lane !== props.item.lane;
  const ok = await put({
    title: draft.title.trim(), body: draft.body.trim(), tags: tags.value, status: draft.status.trim(),
    lane: draft.lane, source: draft.source.trim(), public: draft.public,
    ...(moved ? { position: props.nextPosition?.[draft.lane] ?? 0 } : {}),
  }, 'Saved.');
  if (ok) editing.value = false;
}
const togglePublic = () => put({ public: !props.item.public }, props.item.public ? 'Hidden from the public roadmap.' : 'Shown on the public roadmap.');
async function remove() {
  busy.value = true;
  try {
    await api('DELETE', `/roadmap/items/${props.item.id}`);
    toast(`Deleted "${props.item.title}".`);
    emit('changed');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
    deleting.value = false;
  }
}
</script>

<style scoped>
.item { background: var(--panel); border: 1px solid var(--line); border-radius: 12px; padding: 14px 16px; display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.item.hidden { border-style: dashed; }
.item.editing { border-color: var(--accent); }
.item h3 { font: 600 16px/1.25 var(--sans); letter-spacing: 0; }
.item p { color: var(--soft); font-size: 14px; overflow-wrap: anywhere; }
.meta { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.meta .pill { text-transform: uppercase; font-size: 10px; letter-spacing: 0.06em; }
.hidden-pill { background: transparent; border: 1px dashed var(--line-2); }
.from { font: 12px var(--mono); color: var(--faint); }
.acts { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin-top: 2px; }
.acts .spacer { flex: 1 1 auto; }
.acts .icon { padding: 6px 8px; min-width: 28px; justify-content: center; }
.confirm { gap: 6px; }
.two { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.tick { display: inline-flex; gap: 8px; align-items: center; color: var(--soft); font-size: 13px; }
.edit textarea { font-family: var(--sans); font-size: 14px; min-height: 80px; }
.warn-text { color: var(--warn); }
</style>
