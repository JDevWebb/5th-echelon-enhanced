<template>
  <div class="page">
    <div class="inner">
      <p class="kicker">{{ tab === 'roadmap' ? "What's being built" : 'From your server' }}</p>
      <div class="head">
        <h1 class="title">Intel</h1>
        <div class="tabs" role="tablist">
          <button type="button" role="tab" :aria-selected="tab === 'news'" :class="{ on: tab === 'news' }" @click="tab = 'news'">Briefings</button>
          <button v-if="info.community" type="button" role="tab" :aria-selected="tab === 'roadmap'" :class="{ on: tab === 'roadmap' }" @click="tab = 'roadmap'">Roadmap</button>
        </div>
      </div>

      <Transition name="view" mode="out-in">
        <!-- Briefings: the server's news. -->
        <section v-if="tab === 'news'" key="news">
          <p v-if="!info.game_dir" class="muted">Find the game first (Play).</p>
          <p v-else-if="!info.server" class="muted">Choose a server first: its news shows here.</p>
          <template v-else>
            <p class="muted">From {{ info.server }}, as the game's news screen shows it</p>
            <p v-if="!news" class="soft">Fetching the news…</p>
            <p v-else-if="!news.length" class="soft">No news from this server.</p>
            <div v-else class="briefings" :class="{ single: news.length === 1 }">
              <div v-if="news.length > 1" class="card list">
                <button v-for="(n, i) in news" :key="i" type="button" :class="{ on: i === open }" @click="open = i">{{ n.title }}</button>
              </div>
              <Transition name="view" mode="out-in">
                <article :key="open" class="card brief">
                  <p class="kicker">Briefing {{ open + 1 }} of {{ news.length }}</p>
                  <h2>{{ news[open].title }}</h2>
                  <p v-if="news[open].text" class="soft">{{ news[open].text }}</p>
                  <!-- Only a web address opens, and the player sees where it goes. -->
                  <a v-if="news[open].link.startsWith('https://')" href="#" :title="news[open].link" @click.prevent="call('open_url', { url: news[open].link })">Read more</a>
                </article>
              </Transition>
            </div>
          </template>
        </section>

        <!-- The roadmap, the suggestion form and the player's suggestions. -->
        <section v-else key="roadmap">
          <p class="soft">{{ roadmap?.as_of ? `What's being built for 5th Echelon Enhanced, as of ${roadmap.as_of}.` : "What's being built for 5th Echelon Enhanced, and what's next." }}</p>
          <p v-if="!roadmap" class="muted working"><i class="spin"></i>Reading the roadmap…</p>
          <template v-else>
            <p v-if="roadmap.error" class="muted small">{{ roadmap.error }}{{ roadmap.lanes.length ? ' This is the roadmap as last read.' : '' }}</p>
            <p v-if="roadmap.lanes.length && !main.length && !requested.some((l) => l.items.length)" class="muted">Nothing on the roadmap yet.</p>
            <div class="lanes">
              <div v-for="lane in main" :key="lane.id" class="lane">
                <p class="lane-head"><span class="caps">{{ lane.title }}</span><span v-if="lane.release" class="rel">{{ lane.release }}</span><span class="muted small">{{ lane.items.length }}</span></p>
                <p v-if="!lane.items.length" class="muted">Nothing here yet.</p>
                <div v-for="item in lane.items" :key="item.id" class="item" :class="{ shipping: lane.id === 'shipping' }">
                  <b>{{ item.title }}</b>
                  <p v-if="item.body" class="soft">{{ item.body }}</p>
                  <p v-if="item.status || item.tags.length" class="chips">
                    <span v-if="item.status" class="chip" :class="{ ship: lane.id === 'shipping' }">{{ item.status }}</span>
                    <span v-for="t in item.tags" :key="t" class="chip muted">{{ t }}</span>
                  </p>
                </div>
              </div>
            </div>
            <template v-for="lane in requested.filter((l) => l.items.length)" :key="lane.id">
              <p class="lane-head"><span class="caps">{{ lane.title }}</span><span class="muted small">{{ lane.items.length }}</span></p>
              <div class="requested">
                <div v-for="item in lane.items" :key="item.id" class="item">
                  <b>{{ item.title }}</b>
                  <p v-if="item.body" class="soft">{{ item.body }}</p>
                </div>
              </div>
            </template>
          </template>

          <section class="card form">
            <h2>Suggest a feature or improvement</h2>
            <p class="muted">It goes to the project's admins, with your name and server. You'll see their answer below.</p>
            <p v-if="form && !form.identity" class="soft">Connect to a server first: suggestions are signed with the identity the launcher makes then.</p>
            <template v-else-if="form">
              <p class="caps">About</p>
              <div class="seg">
                <button v-for="(a, i) in form.areas" :key="a" type="button" :class="{ on: area === i }" @click="area = i">{{ a }}</button>
              </div>
              <p class="caps">Title</p>
              <input v-model="title" class="field" type="text" placeholder="In a few words" :maxlength="form.title_max" autocomplete="off" />
              <p class="caps row-between"><span>Details</span><span class="muted small">{{ text.length }}/{{ form.text_max }}</span></p>
              <textarea v-model="text" rows="5" :maxlength="form.text_max" placeholder="What would you like, and what would it help with?"></textarea>
              <div class="send">
                <button type="button" class="btn primary" :disabled="!!why" @click="send">Send</button>
                <span v-if="why && (title.trim() || text.trim())" class="muted">{{ why }}</span>
              </div>
            </template>
          </section>

          <h2 class="mine-head">Your suggestions <i v-if="mineLoading" class="spin"></i></h2>
          <p v-if="mineError" class="muted">{{ mineError }}</p>
          <p v-else-if="mine && !mine.length" class="muted">None yet. What you send shows here, with the admins' answer.</p>
          <div v-for="s in mine ?? []" :key="s.title + s.line" class="suggestion">
            <p class="row-between"><b>{{ s.title }}</b><span class="chip" :class="s.status.toLowerCase()">{{ s.status }}</span></p>
            <p class="muted small">{{ s.line }}</p>
            <div v-if="s.reply" class="reply"><p class="caps">Reply</p><p class="soft">{{ s.reply }}</p></div>
          </div>
        </section>
      </Transition>
    </div>
  </div>
</template>

<script setup>
// Intel (the egui launcher's News and Roadmap screens): the server's briefings, and on the
// community network what's being built, with suggestions signed by the player's identity.
import { computed, onMounted, ref, watch } from 'vue';
import { call } from '../lib/tauri.js';

const props = defineProps({ info: { type: Object, required: true } });

const tab = ref('news');
const news = ref(null);
const open = ref(0);
const roadmap = ref(null);
const form = ref(null);
const area = ref(0);
const title = ref('');
const text = ref('');
const why = ref(null);
const mine = ref(null);
const mineError = ref(null);
const mineLoading = ref(false);

const main = computed(() => (roadmap.value?.lanes ?? []).filter((l) => l.id !== 'requested'));
const requested = computed(() => (roadmap.value?.lanes ?? []).filter((l) => l.id === 'requested'));

async function loadRoadmap() {
  form.value ??= await call('suggestion_form');
  roadmap.value = await call('roadmap');
  loadMine();
}

async function loadMine() {
  mineLoading.value = true;
  try {
    mine.value = await call('my_suggestions');
    mineError.value = null;
  } catch (e) {
    mineError.value = String(e);
  } finally {
    mineLoading.value = false;
  }
}

watch(
  [area, title, text],
  async () => {
    why.value = await call('check_suggestion', { area: area.value, title: title.value, text: text.value });
  },
  { immediate: true },
);

async function send() {
  try {
    await call('suggest', { area: area.value, title: title.value, text: text.value });
    title.value = '';
    text.value = '';
  } catch (e) {
    why.value = String(e);
  }
}

watch(tab, (t) => t === 'roadmap' && !roadmap.value && loadRoadmap());
onMounted(async () => {
  news.value = await call('news');
});
defineExpose({ show: (t) => (tab.value = t) });
</script>

<style scoped>
.head { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; margin-bottom: 10px; }
.tabs { display: flex; gap: 4px; padding: 4px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.tabs button { height: 34px; padding: 0 16px; border-radius: 4px; font: 11.5px PlexMono, monospace; letter-spacing: .14em; text-transform: uppercase; color: var(--muted); transition: background .15s, color .15s; }
.tabs button.on { background: var(--control); color: var(--fg); }
section > p { margin: 0 0 12px; }
.small { font-size: 12.5px; }
.briefings { display: grid; grid-template-columns: 320px 1fr; gap: 16px; align-items: start; }
.briefings.single { grid-template-columns: 1fr; }
.list { padding: 0; overflow: hidden; }
.list button { display: block; width: 100%; min-height: 52px; padding: 10px 16px; text-align: left; font-weight: 600; color: var(--soft); transition: background .15s; }
.list button.on { background: var(--selected); color: var(--fg); box-shadow: inset 3px 0 0 var(--accent); }
.list button:hover:not(.on) { background: var(--control); }
.brief { padding: 24px 28px; }
.brief h2 { margin: 4px 0 6px; font: 700 30px/1.05 PlexCond, sans-serif; text-transform: uppercase; }
.brief p { font-size: 16px; margin: 0 0 10px; }
@media (max-width: 860px) { .briefings { grid-template-columns: 1fr; } }
.lanes { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 16px; margin-bottom: 16px; }
.lane-head { display: flex; align-items: center; gap: 10px; margin: 0 0 6px; }
.lane-head .small { margin-left: auto; }
.rel { font: 12.5px PlexMono, monospace; color: var(--accent); }
.item { padding: 12px 16px; margin-bottom: 8px; border-radius: 6px; background: var(--surface); border: 1px solid var(--line); }
.item.shipping { border-color: rgba(155, 224, 90, .35); }
.item b { font-weight: 600; font-size: 15px; }
.item p { margin: 4px 0 0; font-size: 13.5px; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chip { padding: 0 7px; border-radius: 4px; font-size: 11.5px; line-height: 18px; border: 1px solid var(--control-line); }
.chip.ship, .chip.planned { color: var(--accent); border-color: rgba(155, 224, 90, .45); }
.chip.done { color: var(--accent); border-color: var(--accent); }
.chip.declined { color: var(--muted); }
.requested { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 0 16px; margin-bottom: 16px; }
.form { margin: 26px 0; }
.form h2, .mine-head { margin: 0 0 4px; font-size: 19px; font-weight: 600; display: flex; align-items: center; gap: 10px; }
.form > p { margin: 0 0 8px; }
.form .caps { margin: 12px 0 6px; }
.seg { display: flex; flex-wrap: wrap; gap: 4px; padding: 4px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); width: fit-content; }
.seg button { height: 34px; padding: 0 12px; border-radius: 4px; color: var(--muted); }
.seg button.on { background: var(--control); color: var(--fg); font-weight: 600; }
.field, textarea { width: 100%; padding: 8px 12px; border-radius: 4px; background: var(--sunken); border: 1px solid var(--control-line); outline: none; }
.field { height: 38px; }
.field:focus, textarea:focus { border-color: var(--accent); }
textarea { resize: vertical; }
.row-between { display: flex; justify-content: space-between; align-items: center; gap: 10px; }
.send { display: flex; align-items: center; gap: 12px; margin-top: 10px; }
.btn:disabled { opacity: .45; cursor: default; }
.suggestion { padding: 12px 16px; margin-bottom: 8px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); }
.suggestion p { margin: 0; }
.reply { margin-top: 6px; padding: 8px 12px; border-radius: 6px; background: rgba(155, 224, 90, .06); }
.working { display: flex; align-items: center; gap: 10px; }
.spin { display: inline-block; width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--control-line); border-top-color: var(--accent); animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
