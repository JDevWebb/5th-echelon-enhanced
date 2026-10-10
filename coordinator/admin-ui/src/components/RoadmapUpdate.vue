<template>
  <div class="stack road-update">
    <h2>Update from the project's roadmap</h2>
    <p class="muted">Shipping, Next and Later become the project's roadmap for {{ STARTING_RELEASES.shipping }}. Requested, and items made from players' suggestions, stay as they are.</p>

    <p v-if="!anything" class="small">This roadmap already matches the project's. Nothing to change.</p>
    <div v-else class="changes">
      <section v-if="diff.added.length" aria-labelledby="ru-added">
        <h3 id="ru-added"><span class="pill ok">Added</span> {{ diff.added.length }}</h3>
        <ul><li v-for="i in diff.added" :key="i.title"><b>{{ i.title }}</b> <span class="faint">in {{ laneTitle(i.lane) }}</span></li></ul>
      </section>
      <section v-if="diff.moved.length" aria-labelledby="ru-moved">
        <h3 id="ru-moved"><span class="pill info">Moved</span> {{ diff.moved.length }}</h3>
        <ul>
          <li v-for="m in diff.moved" :key="m.item.title">
            <b>{{ m.item.title }}</b> <span class="faint">{{ laneTitle(m.from) }} → {{ laneTitle(m.item.lane) }}<template v-if="m.fields.length">, new {{ m.fields.join(', ') }}</template></span>
          </li>
        </ul>
      </section>
      <section v-if="diff.changed.length" aria-labelledby="ru-changed">
        <h3 id="ru-changed"><span class="pill warn">Changed</span> {{ diff.changed.length }}</h3>
        <ul><li v-for="c in diff.changed" :key="c.item.title"><b>{{ c.item.title }}</b> <span class="faint">new {{ c.fields.join(', ') }}</span></li></ul>
      </section>
      <section v-if="diff.removed.length" aria-labelledby="ru-removed">
        <h3 id="ru-removed"><span class="pill bad">Removed</span> {{ diff.removed.length }}</h3>
        <ul><li v-for="i in diff.removed" :key="i.id"><b>{{ i.title }}</b> <span class="faint">from {{ laneTitle(i.lane) }}</span></li></ul>
      </section>
      <section v-if="releases.length" aria-labelledby="ru-releases">
        <h3 id="ru-releases"><span class="pill">Releases</span></h3>
        <ul><li v-for="r in releases" :key="r.lane"><b>{{ laneTitle(r.lane) }}</b> <span class="faint">{{ r.from || 'none' }} → {{ r.to || 'none' }}</span></li></ul>
      </section>
    </div>
    <p v-if="diff.same.length || diff.kept.length" class="small faint">
      <template v-if="diff.same.length">{{ diff.same.length }} {{ diff.same.length === 1 ? 'item stays' : 'items stay' }} as {{ diff.same.length === 1 ? 'it is' : 'they are' }}.</template>
      <template v-if="diff.kept.length"> {{ diff.kept.length }} from players' suggestions {{ diff.kept.length === 1 ? 'stays' : 'stay' }}, after the project's in {{ diff.kept.length === 1 ? 'its lane' : 'their lanes' }}.</template>
    </p>

    <div class="row end">
      <button class="ghost" type="button" @click="closeModal(false)">{{ anything ? 'Cancel' : 'Close' }}</button>
      <button v-if="anything" :class="diff.removed.length ? 'danger' : 'primary'" type="button" :disabled="busy" @click="apply">{{ busy ? 'Updating…' : 'Update the roadmap' }}</button>
    </div>
  </div>
</template>

<script setup>
// The preview of bringing the roadmap up to the project's: what's added, moved, changed and
// removed. Nothing changes until the admin confirms; then it's one request.
import { computed, ref } from 'vue';
import { api } from '../lib/api.js';
import { STARTING_RELEASES, projectChanges, projectRoadmap } from '../lib/roadmap.js';
import { closeModal, toast } from '../lib/ui.js';

const props = defineProps({ items: Array, lanes: Array });
const diff = computed(() => projectChanges(props.items));
const laneTitle = id => props.lanes?.find(l => l.id === id)?.title || id;
const releases = computed(() => Object.entries(STARTING_RELEASES)
  .map(([lane, to]) => ({ lane, to, from: props.lanes?.find(l => l.id === lane)?.release || '' }))
  .filter(r => r.from !== r.to));
const anything = computed(() => diff.value.added.length + diff.value.moved.length + diff.value.changed.length + diff.value.removed.length + releases.value.length > 0);

const busy = ref(false);
async function apply() {
  busy.value = true;
  try {
    closeModal(await api('POST', '/roadmap/from-project', projectRoadmap()));
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
</script>

<style scoped>
.changes { display: flex; flex-direction: column; gap: 14px; max-height: min(52vh, 460px); overflow-y: auto; padding-right: 4px; }
h3 { display: flex; align-items: center; gap: 8px; font: 400 12px var(--mono); color: var(--muted); margin-bottom: 6px; }
ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
li { font-size: 13px; line-height: 1.4; }
li b { font-weight: 600; color: var(--text); }
:global(dialog:has(.road-update)) { width: min(560px, calc(100vw - 32px)); }
</style>
