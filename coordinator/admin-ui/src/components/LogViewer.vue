<template>
  <div class="viewer">
    <div class="row bar">
      <input v-model="find" type="search" placeholder="Find in this log" :aria-label="`Find in ${name}`">
      <span class="small muted count">{{ find.trim() ? `${fmt.n(shown.length)} of ${fmt.n(lines.length)} lines` : `${fmt.n(lines.length)} lines` }}</span>
      <button class="small" type="button" @click="copy">Copy</button>
    </div>
    <p v-if="cut" class="small muted">Showing the first 2 MB; download the file for all of it.</p>
    <pre class="log" tabindex="0" :aria-label="name">{{ shownText }}</pre>
    <p v-if="find.trim() && !shown.length" class="small muted">Nothing matches.</p>
  </div>
</template>

<script setup>
// A log, monospace and scrollable, with a search that keeps the lines that match.
import { computed, ref } from 'vue';
import { fmt } from '../lib/fmt.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ text: { type: String, default: '' }, name: String, cut: Boolean });
const find = ref('');
const lines = computed(() => props.text.replace(/\n$/, '').split('\n').map((text, i) => ({ n: i + 1, text })));
const shown = computed(() => {
  const f = find.value.trim().toLowerCase();
  return f ? lines.value.filter(l => l.text.toLowerCase().includes(f)) : lines.value;
});
// One text node, line numbers in it: a few MB of log renders at once.
const shownText = computed(() => {
  const width = String(lines.value.length).length;
  return shown.value.map(l => `${String(l.n).padStart(width)}  ${l.text}`).join('\n');
});
async function copy() {
  try {
    await navigator.clipboard.writeText(props.text);
    toast('Copied.');
  } catch { toast('The browser didn\'t allow copying.', true); }
}
</script>

<style scoped>
.viewer { display: flex; flex-direction: column; gap: 8px; }
.bar input { flex: 1 1 200px; width: auto; padding: 6px 10px; font-size: 13px; }
.count { white-space: nowrap; }
.log {
  margin: 0; max-height: 420px; overflow: auto;
  background: var(--bg); border: 1px solid var(--line); border-radius: 8px;
  padding: 10px 12px; font: 12px/1.55 var(--mono); color: var(--soft); white-space: pre;
}
</style>
