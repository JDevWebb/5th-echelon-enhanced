<template>
  <PageTop title="Roadmap" sub="What's shipping, what's next, and what players and admins have asked for. Each item can hand you a prompt to research and build it.">
    <template v-if="road" #stats>
      <Stat :value="laneOf('shipping')?.release || '–'" label="Releasing" :sub="`${count('shipping')} shipping`" />
      <Stat :value="fmt.n(count('next'))" :label="laneOf('next')?.release ? `Next, ${laneOf('next').release}` : 'Next'" />
      <Stat :value="fmt.n(count('later'))" label="Later" />
      <Stat :value="fmt.n(count('requested'))" label="Requested" />
      <Stat :value="fmt.n(road.suggestions?.new || 0)" label="New suggestions" :tone="road.suggestions?.new ? 'info' : ''" />
    </template>
    <button class="primary" type="button" @click="jump('suggest', 's-title')">Suggest</button>
  </PageTop>

  <div v-if="error" class="panel empty">
    <p>The roadmap isn't available on this coordinator yet.</p>
    <p class="small err">{{ error }}</p>
  </div>
  <div v-else-if="!road" class="empty">Loading…</div>
  <section v-else-if="!road.items.length" class="panel start">
    <h2 class="big-title">Nothing on the roadmap yet</h2>
    <p class="muted">Start from the project's roadmap as it stood for 0.4.2: {{ STARTING_ITEMS.length }} items in four lanes. Players see all but the requests, which quote players. Edit or delete any of them afterwards.</p>
    <div class="row">
      <button class="primary" type="button" :disabled="seeding" @click="seed">{{ seeding ? `Adding ${seeded} of ${STARTING_ITEMS.length}…` : 'Start from the project\'s roadmap' }}</button>
      <span class="small muted">Or add your own with the form below.</span>
    </div>
  </section>
  <div v-else class="lanes">
    <section v-for="lane in lanes" :key="lane.id" class="lane" :aria-labelledby="`lane-${lane.id}`">
      <div class="lane-head">
        <h2 :id="`lane-${lane.id}`">{{ lane.title }}</h2>
        <form v-if="releaseFor === lane.id" class="row release-form" @submit.prevent="saveRelease(lane)">
          <input v-model="releaseDraft" maxlength="24" placeholder="0.4.3" :aria-label="`${lane.title} release`">
          <button class="primary small" type="submit">Save</button>
          <button class="ghost small" type="button" @click="releaseFor = null">Cancel</button>
        </form>
        <button v-else class="ghost small release" type="button" :title="`Set ${lane.title}'s release`" @click="editRelease(lane)">{{ lane.release || 'No release' }} · {{ lane.items.length }}</button>
      </div>
      <RoadmapItem
        v-for="(item, i) in lane.items" :key="item.id" :item="item" :lanes="road.lanes" :first="i === 0" :last="i === lane.items.length - 1"
        :next-position="nextPosition" @prompt="showPrompt('item', $event)" @move="move(lane, i, $event)" @changed="reload" />
      <p v-if="!lane.items.length" class="lane-empty">Nothing here.</p>
    </section>
  </div>

  <section id="suggest" class="panel suggest" aria-labelledby="suggest-h">
    <div>
      <h2 id="suggest-h" class="big-title">Suggest a feature or improvement</h2>
      <p class="lead">Write down what a player or admin asked for. It goes under Requested, and you get a prompt to start on it.</p>
      <form class="stack" @submit.prevent="suggest">
        <label class="field"><span>What</span><input id="s-title" v-model="form.title" required maxlength="80" placeholder="Show who's in each match on the server card"></label>
        <label class="field"><span>Why, or in their words</span><textarea v-model="form.body" required maxlength="600" placeholder="&quot;I can't tell which server my friends are on before I join.&quot;"></textarea></label>
        <div class="two">
          <label class="field"><span>Area</span><select v-model="form.area"><option v-for="a in AREAS" :key="a">{{ a }}</option></select></label>
          <label class="field"><span>Asked by</span><input v-model="form.from" maxlength="44" placeholder="Discord · Kiwi"></label>
        </div>
        <label class="tick"><input v-model="form.public" type="checkbox"> Show it on the public roadmap</label>
        <div class="row">
          <button class="primary" type="submit" :disabled="adding">Add to Requested</button>
          <span class="small faint">Saved for every admin, and in the audit log.</span>
        </div>
      </form>
    </div>
    <div id="prompt" class="prompt">
      <span class="label">Prompt{{ promptTitle ? ` · ${promptTitle}` : '' }}</span>
      <pre ref="pre" tabindex="0" aria-label="Prompt for Claude Code">{{ promptText || 'Pick Prompt on any item or suggestion, or add a request, and the prompt to start on it appears here.' }}</pre>
      <div class="row">
        <button type="button" :disabled="!promptText" @click="copy">Copy prompt</button>
        <span v-if="copied" class="copied" role="status">{{ copied }}</span>
        <span class="small faint">Paste it into Claude Code in the repository.</span>
      </div>
    </div>
  </section>

  <section class="panel" aria-labelledby="inbox-h">
    <header>
      <h2 id="inbox-h">Suggestions from players</h2>
      <div class="seg" role="group" aria-label="Show">
        <button type="button" :aria-pressed="String(filter === 'new')" @click="filter = 'new'">New</button>
        <button type="button" :aria-pressed="String(filter === 'all')" @click="filter = 'all'">All</button>
      </div>
    </header>
    <p class="small muted inbox-lead">Sent from the launcher's roadmap, signed by the player's identity. Your reply and status show in their launcher. Promote one to put it on the roadmap, hidden until you make it public.</p>
    <div v-if="sugError" class="empty">
      <p>Suggestions aren't available on this coordinator yet.</p>
      <p class="small err">{{ sugError }}</p>
    </div>
    <div v-else-if="!sugs" class="empty">Loading…</div>
    <div v-else-if="!sugs.suggestions.length" class="empty">{{ filter === 'new' ? 'No new suggestions.' : 'No suggestions yet. Players send them from the launcher\'s roadmap.' }}</div>
    <div v-else class="inbox">
      <SuggestionCard v-for="s in sugs.suggestions" :key="s.id" :s="s" @prompt="showPrompt('suggestion', $event)" @changed="reloadAll" />
    </div>
  </section>
</template>

<script setup>
// The roadmap (four lanes of items admins keep, the public ones shown in launchers), the
// players' suggestions, and a prompt for Claude Code to start on any of them.
import { computed, nextTick, reactive, ref, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import RoadmapItem from '../components/RoadmapItem.vue';
import SuggestionCard from '../components/SuggestionCard.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { AREAS, itemPrompt, LANE_IDS, STARTING_ITEMS, STARTING_RELEASES, suggestionPrompt } from '../lib/roadmap.js';
import { toast } from '../lib/ui.js';

const { data: road, error, reload } = useLoad(() => api('GET', '/roadmap'), () => null, { refresh: false });
const filter = ref('new');
const { data: sugs, error: sugError, reload: reloadSugs } = useLoad(() => api('GET', `/suggestions?status=${filter.value}`), () => filter.value, { refresh: false });
const reloadAll = () => { reload(); reloadSugs(); };
// Any admin's change, here or in another tab.
watch(() => live.roadmapTick, reloadAll);

const byOrder = (a, b) => ((a.position || 0) - (b.position || 0)) || (a.id - b.id);
const lanes = computed(() => {
  const order = [...(road.value?.lanes || [])].sort((a, b) => LANE_IDS.indexOf(a.id) - LANE_IDS.indexOf(b.id));
  return order.map(l => ({ ...l, items: (road.value.items || []).filter(i => i.lane === l.id).sort(byOrder) }));
});
const laneOf = id => lanes.value.find(l => l.id === id);
const count = id => laneOf(id)?.items.length || 0;
/** Where an item moved to each lane goes: after the last one there. */
const nextPosition = computed(() => Object.fromEntries(lanes.value.map(l => [l.id, l.items.length ? Math.max(...l.items.map(i => i.position || 0)) + 1 : 0])));

// Moving: the lane's items in their new order, numbered 0, 1, 2 … (only those that change are saved).
async function move(lane, i, dir) {
  const items = [...lane.items];
  const j = i + dir;
  if (j < 0 || j >= items.length) return;
  [items[i], items[j]] = [items[j], items[i]];
  try {
    for (const [n, it] of items.entries()) {
      if (it.position === n) continue;
      await api('PUT', `/roadmap/items/${it.id}`, { lane: it.lane, title: it.title, body: it.body || '', tags: it.tags || [], status: it.status || '', public: it.public, source: it.source || '', position: n });
    }
  } catch (e) { toast(e.message, true); }
  reload();
}

// Lane releases.
const releaseFor = ref(null);
const releaseDraft = ref('');
function editRelease(lane) {
  releaseFor.value = lane.id;
  releaseDraft.value = lane.release || '';
}
async function saveRelease(lane) {
  const release = releaseDraft.value.trim();
  try {
    await api('PUT', `/roadmap/lanes/${lane.id}`, { release });
    toast(release ? `${lane.title} is ${release}.` : `${lane.title} has no release now.`);
    releaseFor.value = null;
    reload();
  } catch (e) { toast(e.message, true); }
}

// An empty roadmap starts from the project's.
const seeding = ref(false);
const seeded = ref(0);
async function seed() {
  seeding.value = true;
  seeded.value = 0;
  try {
    for (const item of STARTING_ITEMS) {
      await api('POST', '/roadmap/items', item);
      seeded.value++;
    }
    for (const [lane, release] of Object.entries(STARTING_RELEASES)) await api('PUT', `/roadmap/lanes/${lane}`, { release });
    toast('The project\'s roadmap is in. Edit anything to suit.');
  } catch (e) {
    toast(`Stopped after ${seeded.value} items: ${e.message}`, true);
  } finally {
    seeding.value = false;
    reload();
  }
}

// The suggest form: an admin's own request, under Requested.
const form = reactive({ title: '', body: '', area: AREAS[0], from: '', public: false });
const adding = ref(false);
async function suggest() {
  const title = form.title.trim();
  const body = form.body.trim();
  if (!title || !body) return;
  const when = new Date().toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
  const fields = { lane: 'requested', title, body, tags: [form.area], status: 'Requested', public: form.public, source: `${form.from.trim() || 'Admin'} · ${when}` };
  adding.value = true;
  try {
    const item = await api('POST', '/roadmap/items', fields);
    toast('Added under Requested.');
    Object.assign(form, { title: '', body: '', from: '', public: false });
    showPrompt('item', item?.title ? item : fields, false);
    reload();
  } catch (e) {
    toast(e.message, true);
  } finally {
    adding.value = false;
  }
}

// The prompt panel.
const promptFor = ref(null);
const pre = ref(null);
const copied = ref('');
const promptTitle = computed(() => promptFor.value?.what.title || '');
const promptText = computed(() => {
  const p = promptFor.value;
  if (!p) return '';
  return p.kind === 'suggestion' ? suggestionPrompt(p.what) : itemPrompt(p.what);
});
function showPrompt(kind, what, scroll = true) {
  promptFor.value = { kind, what };
  copied.value = '';
  if (scroll) jump('prompt');
}
async function copy() {
  try {
    await navigator.clipboard.writeText(promptText.value);
    copied.value = 'Copied';
  } catch {
    // No clipboard here (an older browser, or not allowed): select it to copy by hand.
    const range = document.createRange();
    range.selectNodeContents(pre.value);
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    copied.value = 'Selected: press Ctrl+C or ⌘C to copy';
  }
}

function jump(id, focus) {
  nextTick(() => {
    const el = document.getElementById(id);
    if (!el) return;
    el.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
    if (focus) document.getElementById(focus)?.focus({ preventScroll: true });
  });
}
</script>

<style scoped>
.lanes { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 16px; align-items: start; }
.lane { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.lane-head { display: flex; justify-content: space-between; align-items: center; gap: 8px; padding: 0 2px; min-height: 30px; flex-wrap: wrap; }
.lane-head h2 { font: 400 13px var(--mono); letter-spacing: 0.1em; text-transform: uppercase; color: var(--soft); }
.release { font: 12px var(--mono); color: var(--faint); padding: 4px 6px; }
.release-form { gap: 6px; flex-wrap: nowrap; }
.release-form input { width: 80px; padding: 5px 8px; font-size: 13px; }
.lane-empty { color: var(--faint); font-size: 13px; border: 1px dashed var(--line); border-radius: 12px; padding: 14px 16px; }
.start { display: flex; flex-direction: column; gap: 12px; }
.suggest { display: grid; grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr); gap: 22px; padding: 20px 22px; scroll-margin-top: 16px; }
.suggest .lead { color: var(--soft); margin: 4px 0 14px; }
.suggest textarea { font-family: var(--sans); font-size: 14px; min-height: 84px; }
.two { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
.tick { display: inline-flex; gap: 8px; align-items: center; color: var(--soft); font-size: 13px; }
.prompt { display: flex; flex-direction: column; gap: 8px; min-width: 0; scroll-margin-top: 16px; }
.prompt pre {
  margin: 0; background: var(--rail); border: 1px solid var(--line); border-radius: 10px; padding: 14px;
  font: 13px/1.55 var(--mono); color: var(--soft); white-space: pre-wrap; word-break: break-word; min-height: 220px;
}
.copied { color: var(--ok); font: 12px var(--mono); }
.inbox-lead { margin: -4px 0 14px; max-width: 80ch; }
.inbox { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
@media (max-width: 1200px) { .lanes { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@media (max-width: 900px) { .inbox { grid-template-columns: minmax(0, 1fr); } }
@media (max-width: 760px) {
  .lanes, .suggest, .two { grid-template-columns: minmax(0, 1fr); }
}
</style>
