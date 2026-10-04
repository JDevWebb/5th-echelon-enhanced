<template>
  <section class="panel detail">
    <header>
      <div>
        <h2 class="row" style="gap: 8px">{{ r?.player.name || '…' }} <RatingPill v-if="r" :rating="r.rating" /></h2>
        <p v-if="r" class="small muted">{{ names.get(r.server) || r.server }} · #{{ r.player.id }} · {{ fmt.when(r.created_at) }}</p>
      </div>
      <button class="ghost small" type="button" aria-label="Close" @click="$emit('close')">✕</button>
    </header>
    <div v-if="!r" class="empty">{{ error || 'Loading…' }}</div>
    <template v-else>
      <div v-if="r.problems.length" class="tags"><span v-for="p in r.problems" :key="p" class="pill warn">{{ PROBLEMS[p] || p }}</span></div>
      <blockquote v-if="r.comment" class="said">{{ r.comment }}</blockquote>
      <p v-else class="small muted">No comment.</p>

      <p class="small player">
        <RouterLink v-if="r.player_known" :to="`/players/${encodeURIComponent(r.server)}/${r.player.id}`">{{ r.player.name }}'s account</RouterLink>
        <span v-else class="muted">Not in {{ names.get(r.server) || r.server }}'s player list (yet, or any more).</span>
        <span class="muted">{{ r.player.identity ? ` · identity ${r.player.identity.slice(0, 12)}…` : ' · no identity' }}</span>
      </p>

      <div class="status" :class="{ resolved: r.status === 'resolved' }">
        <div class="row" style="justify-content: space-between">
          <span>
            <span class="pill" :class="r.status === 'resolved' ? 'ok' : 'warn'">{{ r.status }}</span>
            <span v-if="r.status === 'resolved'" class="small muted"> by {{ r.resolved_by }} · {{ fmt.ago(r.resolved_at) }}</span>
          </span>
          <button class="small danger" type="button" @click="remove">Delete</button>
        </div>
        <label class="field"><span>Note</span><textarea v-model="note" maxlength="500" rows="2" placeholder="What it was, what was done"></textarea></label>
        <div class="row end">
          <button v-if="note.trim() !== r.note" class="small" type="button" :disabled="busy" @click="setStatus(r.status)">Save note</button>
          <button v-if="r.status === 'open'" class="small primary" type="button" :disabled="busy" @click="setStatus('resolved')">Resolve</button>
          <button v-else class="small" type="button" :disabled="busy" @click="setStatus('open')">Reopen</button>
        </div>
      </div>

      <h3 class="sub">Why the launcher asked</h3>
      <div v-if="r.triggers.length" class="tags"><code v-for="t in r.triggers" :key="t" class="trigger">{{ t }}</code></div>
      <p v-else class="small muted">Not said.</p>

      <h3 class="sub">Client</h3>
      <div v-if="Object.keys(r.client).length" class="facts client">
        <div v-for="(v, k) in r.client" :key="k" class="fact"><b>{{ v }}</b><span>{{ k }}</span></div>
      </div>
      <p v-else class="small muted">Not said.</p>

      <h3 class="sub">Files</h3>
      <ul v-if="r.files.length" class="files">
        <li v-for="f in r.files" :key="f.name">
          <div class="row" style="justify-content: space-between">
            <span><code>{{ f.name }}</code> <span class="small muted">{{ fmt.bytes(f.size) }}</span></span>
            <span v-if="f.dropped" class="small muted">removed (storage cap)</span>
            <span v-else class="row">
              <button class="small" type="button" :aria-expanded="String(!!viewing[f.name])" @click="toggle(f.name)">{{ viewing[f.name] ? 'Hide' : 'View' }}</button>
              <a class="btn small" :href="fileUrl(f.name)" :download="`${r.id}-${f.name}`">Download</a>
            </span>
          </div>
          <template v-if="viewing[f.name]">
            <p v-if="viewing[f.name].error" class="err">{{ viewing[f.name].error }}</p>
            <p v-else-if="viewing[f.name].loading" class="small muted">Loading…</p>
            <LogViewer v-else :text="viewing[f.name].text" :cut="viewing[f.name].cut" :name="f.name" />
          </template>
        </li>
      </ul>
      <p v-else class="small muted">No logs attached.</p>

      <h3 class="sub">The server's log lines</h3>
      <LogViewer v-if="r.server_log" :text="r.server_log" name="the server's log lines" />
      <p v-else class="small muted">None sent.</p>

      <h3 class="sub">The server's summary</h3>
      <pre v-if="Object.keys(r.summary || {}).length" class="json">{{ JSON.stringify(r.summary, null, 2) }}</pre>
      <p v-else class="small muted">None sent.</p>

      <template v-if="r.others.length">
        <h3 class="sub">Their other reports</h3>
        <ul class="list">
          <li v-for="o in r.others" :key="o.id">
            <button class="ghost small" type="button" @click="$emit('open', o.id)">{{ fmt.when(o.created_at) }}</button>
            <span class="small muted">{{ names.get(o.server) || o.server }} · {{ o.problems.map(p => PROBLEMS[p] || p).join(', ') || o.rating || 'feedback' }}{{ o.status === 'resolved' ? ' · resolved' : '' }}</span>
          </li>
        </ul>
      </template>
    </template>
  </section>
</template>

<script setup>
// One report: what the player said, why they were asked, their logs and the server's, and
// what admins did about it.
import { reactive, ref, watch } from 'vue';
import LogViewer from './LogViewer.vue';
import RatingPill from './RatingPill.vue';
import { api, fetchText } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, PROBLEMS } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

/** The most of a file the page shows. */
const VIEW_LIMIT = 2 * 1024 * 1024;
const props = defineProps({ id: String, names: Map });
const emit = defineEmits(['close', 'changed', 'open']);
const r = ref(null);
const error = ref('');
const note = ref('');
const busy = ref(false);
const viewing = reactive({});

async function load() {
  try {
    r.value = await api('GET', `/reports/${props.id}`);
    note.value = r.value.note;
    error.value = '';
  } catch (e) {
    error.value = e.status === 404 ? 'This report is gone (deleted, or past 90 days).' : e.message;
    r.value = null;
  }
}
watch(() => props.id, () => {
  r.value = null;
  for (const k of Object.keys(viewing)) delete viewing[k];
  load();
}, { immediate: true });
// Another admin resolving it shows here too (a note being typed stays).
watch(() => live.reportTick, async () => {
  const typed = r.value && note.value.trim() !== r.value.note ? note.value : null;
  await load();
  if (typed !== null && r.value) note.value = typed;
});

const fileUrl = name => `/api/reports/${props.id}/files/${encodeURIComponent(name)}`;

async function toggle(name) {
  if (viewing[name]) { delete viewing[name]; return; }
  viewing[name] = { loading: true };
  try {
    const { text, cut } = await fetchText(`/reports/${props.id}/files/${encodeURIComponent(name)}`, VIEW_LIMIT);
    if (viewing[name]) viewing[name] = { text, cut };
  } catch (e) {
    if (viewing[name]) viewing[name] = { error: e.message };
  }
}

async function setStatus(status) {
  busy.value = true;
  const was = r.value.status;
  try {
    const row = await api('POST', `/reports/${props.id}`, { status, note: note.value.trim() });
    Object.assign(r.value, { status: row.status, note: row.note, resolved_by: row.resolved_by, resolved_at: row.resolved_at });
    note.value = row.note;
    toast(status === was ? 'Saved.' : status === 'resolved' ? 'Resolved.' : 'Reopened.');
    emit('changed');
  } catch (e) { toast(e.message, true); }
  busy.value = false;
}

async function remove() {
  const ok = await confirmBox('Delete this report?', `${r.value.player.name}'s report and their logs are deleted for good.`, 'Delete', true);
  if (!ok) return;
  try {
    await api('DELETE', `/reports/${props.id}`);
    toast('Deleted.');
    emit('changed');
    emit('close');
  } catch (e) { toast(e.message, true); }
}
</script>

<style scoped>
.detail { position: sticky; top: 16px; max-height: calc(100vh - 32px); overflow-y: auto; }
.sub { margin: 18px 0 8px; font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); }
.tags { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 10px; }
.said { margin: 0 0 10px; padding: 10px 14px; border-left: 3px solid var(--accent); background: var(--panel-2); border-radius: 0 8px 8px 0; white-space: pre-wrap; overflow-wrap: anywhere; }
.player { margin-bottom: 12px; }
.status { display: flex; flex-direction: column; gap: 10px; padding: 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); }
.status textarea { min-height: 56px; font-family: var(--sans); font-size: 13px; }
.trigger { padding: 2px 8px; border-radius: 6px; background: var(--panel-2); border: 1px solid var(--line); }
.client { margin-top: 0; }
.client b { font-size: 13px; overflow-wrap: anywhere; }
.files { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 10px; }
.files li { display: flex; flex-direction: column; gap: 8px; }
.json { margin: 0; max-height: 360px; overflow: auto; background: var(--bg); border: 1px solid var(--line); border-radius: 8px; padding: 10px 12px; font: 12px/1.5 var(--mono); color: var(--soft); }
.list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; }
.list li { display: flex; align-items: center; gap: 8px; }
a.btn.small { padding: 6px 10px; font-size: 12px; }
@media (max-width: 1100px) { .detail { position: static; max-height: none; } }
</style>
