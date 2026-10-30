<template>
  <Transition name="modal">
    <div v-if="view && !waiting" class="backdrop" @click.self="close">
      <div v-modal="close" class="dialog" role="dialog" aria-modal="true" aria-labelledby="wn-title">
        <p class="top"><span class="caps">Updated</span><span class="mono soft">{{ view.from ? `${view.from} → ${view.to}` : view.to }}</span></p>
        <h2 id="wn-title">What's new</h2>
        <div class="middle">
          <div v-if="newest.thanks" class="thanks">
            <p class="caps accent">Thanks to your reports</p>
            <p>{{ newest.thanks }} sent reports from the launcher: much of this release came from them.</p>
            <p class="muted">Send yours from Settings › Feedback.</p>
          </div>
          <p class="caps">{{ skipped ? `In ${newest.version}` : 'New' }}</p>
          <div v-for="(item, i) in newest.new" :key="item.title" class="item">
            <span class="dot" aria-hidden="true"></span>
            <div>
              <p><b>{{ item.title }}</b><span v-if="item.community" class="badge">Community network</span></p>
              <p class="soft">{{ item.text }}</p>
              <template v-if="!community && item.community && i === firstCommunity">
                <p class="muted small">Your launcher uses another network, which doesn't have these.</p>
                <a href="#" class="small" @click.prevent="useCommunity">Switch to the community network ›</a>
              </template>
            </div>
          </div>
          <template v-if="skipped">
            <p v-if="newest.fixed.length" class="plus">
              Plus {{ newest.fixed.length === 1 ? '1 fix' : `${newest.fixed.length} fixes` }}, among them <b>{{ newest.fixed[0][0] }}</b>.
            </p>
            <hr />
            <template v-for="older in view.releases.slice(1)" :key="older.version">
              <p class="caps">And in {{ older.version }}, which you skipped</p>
              <p v-for="item in older.new.slice(0, view.headlines)" :key="item.title" class="headline"><span class="dot" aria-hidden="true"></span>{{ item.title }}</p>
            </template>
          </template>
          <template v-else-if="newest.fixed.length">
            <p class="caps">Fixed</p>
            <p v-for="[lead, rest] in newest.fixed" :key="lead" class="fixline"><span class="tick" aria-hidden="true">✓</span><span><b>{{ lead }}</b>{{ rest }}</span></p>
          </template>
        </div>
        <hr />
        <div class="actions">
          <a href="#" @click.prevent="call('open_url', { url: view.notes[1] })">{{ view.notes[0] }}</a>
          <button type="button" class="btn primary" @click="close">Got it</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<script setup>
// What's new, once after an update (the egui launcher's whats_new.rs; its releases are the
// shared library's). It waits while the first-run questions are up (`waiting`).
import { computed, onMounted, ref } from 'vue';
import { call } from '../lib/tauri.js';

defineProps({ waiting: Boolean, community: Boolean });
const emit = defineEmits(['network']);

const view = ref(null);
const newest = computed(() => view.value?.releases[0]);
const skipped = computed(() => (view.value?.releases.length ?? 0) > 1);
// Off the community network, the way there once, under its first change.
const firstCommunity = computed(() => newest.value?.new.findIndex((i) => i.community) ?? -1);

async function close() {
  view.value = null;
  await call('whats_new_seen');
}

async function useCommunity() {
  await call('use_community_network');
  await close();
  emit('network');
}

/** This launcher's release again (the version in the side menu): whether there's one. */
async function again() {
  view.value = await call('whats_new_again');
  return Boolean(view.value);
}

defineExpose({ again });
onMounted(async () => {
  view.value = await call('whats_new');
});
</script>

<style scoped>
.backdrop { position: fixed; inset: 0; z-index: 45; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(560px, 100%); max-height: calc(100vh - 60px); display: flex; flex-direction: column; padding: 22px 26px 18px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
.top { display: flex; gap: 10px; align-items: baseline; margin: 0; }
.mono { font: 12px PlexMono, monospace; }
h2 { margin: 2px 0 10px; font: 700 30px/1.05 PlexCond, sans-serif; text-transform: uppercase; }
.middle { overflow: auto; min-height: 0; display: flex; flex-direction: column; gap: 12px; padding-right: 4px; }
.middle p { margin: 0; }
.accent { color: var(--accent); }
.thanks { padding: 12px 14px; border-radius: 6px; background: var(--selected); border: 1px solid rgba(155, 224, 90, .3); display: flex; flex-direction: column; gap: 4px; }
.item { display: flex; gap: 12px; }
.item b { font-weight: 600; }
.dot { flex: none; width: 6px; height: 6px; margin-top: 8px; border-radius: 50%; background: var(--accent); }
.badge { margin-left: 8px; padding: 0 6px; border-radius: 3px; font: 10px/17px PlexMono, monospace; letter-spacing: .08em; text-transform: uppercase; color: var(--accent); border: 1px solid rgba(155, 224, 90, .4); vertical-align: 2px; }
.small { font-size: 13.5px; }
.fixline { display: flex; gap: 10px; }
.fixline b, .plus b { font-weight: 600; }
.tick { color: var(--accent); }
.headline { display: flex; gap: 12px; }
.headline .dot { margin-top: 8px; }
hr { width: 100%; border: 0; border-top: 1px solid var(--line); margin: 6px 0; }
.actions { display: flex; align-items: center; justify-content: space-between; margin-top: 4px; }
.actions .btn { min-width: 120px; height: 40px; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-from, .modal-leave-to { opacity: 0; }
</style>
