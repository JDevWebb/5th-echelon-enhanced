<template>
  <section id="notices" class="panel">
    <header>
      <h2 class="big-title">Notices to every player</h2>
      <span class="small muted">Every launcher shows live notices above Play, within a minute.</span>
    </header>
    <p class="small muted lead-in">For what everyone should know now: a known problem, a fix on its way, a fix that's out. Players can close one; an edit shows it to them again. Three live at most.</p>
    <form class="post" @submit.prevent="save">
      <span id="nt-kind" class="label">Kind</span>
      <div class="row" role="group" aria-labelledby="nt-kind">
        <button v-for="[k, text] in KINDS" :key="k" class="chip" type="button" :aria-pressed="String(kind === k)" @click="kind = k">{{ text }}</button>
      </div>

      <label class="label" for="nt-text">Text</label>
      <div class="stack" style="gap: 6px">
        <input id="nt-text" v-model="text" type="text" :maxlength="MAX" :placeholder="PLACEHOLDER[kind]" class="text">
        <span class="small muted">One line, {{ text.trim().length }}/{{ MAX }}.</span>
      </div>

      <span id="nt-ends" class="label">Ends</span>
      <div class="row" role="group" aria-labelledby="nt-ends">
        <button v-for="[h, label] in ENDS" :key="String(h)" class="chip" type="button" :aria-pressed="String(hours === h)" @click="hours = h">{{ label }}</button>
      </div>

      <span class="label">Players see</span>
      <div class="preview" :class="kind">
        <b>{{ HEADING[kind] }}</b> {{ text.trim() || PLACEHOLDER[kind] }}
      </div>

      <div class="row end actions">
        <span v-if="problem" class="small warn-text" role="status">{{ problem }}</span>
        <button v-if="editing" class="ghost" type="button" @click="reset">Stop editing</button>
        <button class="primary" type="submit" :disabled="busy || !!problem">{{ editing ? 'Save the change' : 'Post to every player' }}</button>
      </div>
    </form>

    <div v-if="error" class="empty">
      <p>Notices aren't available here yet.</p>
      <p class="small err">{{ error }}</p>
    </div>
    <div v-else-if="!list" class="empty">Loading…</div>
    <div v-else-if="!list.length" class="empty">No notices yet.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>Kind</th><th>Text</th><th>Posted (yours)</th><th>Status</th><th></th></tr></thead>
        <tbody>
          <tr v-for="n in list" :key="n.id">
            <td><span class="pill" :class="PILL[n.kind]">{{ HEADING[n.kind] }}</span></td>
            <td>{{ n.text }}<span class="sub">by {{ n.posted_by }}{{ n.edited ? `, edited ${when(n.edited)}` : '' }}</span></td>
            <td>{{ when(n.posted) }}</td>
            <td>
              <span class="pill" :class="n.live ? 'ok' : ''">{{ n.live ? 'Live' : 'Ended' }}</span>
              <span class="sub">{{ n.live ? (n.ends ? `ends ${when(n.ends)}` : 'until ended') : (n.ended ? when(n.ended) : '') }}</span>
            </td>
            <td class="r">
              <span v-if="confirming === n.id" class="row end confirm" role="group" aria-label="End this notice?">
                <span class="small">End it?</span>
                <button class="danger small" type="button" :disabled="busy" @click="end(n)">Yes, end</button>
                <button class="ghost small" type="button" @click="confirming = null">Keep</button>
              </span>
              <span v-else-if="n.live" class="row end confirm">
                <button class="ghost small" type="button" @click="edit(n)">Edit</button>
                <button class="ghost small" type="button" @click="confirming = n.id">End</button>
              </span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<script setup>
// Urgent notices (0.5.2's U): what every player's launcher shows above Play.
import { computed, ref, watch } from 'vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const MAX = 200;
const KINDS = [['issue', 'A known problem'], ['info', 'News'], ['fixed', 'Fixed']];
const HEADING = { issue: 'Known issue:', info: 'News:', fixed: 'Fixed:' };
const PILL = { issue: 'warn', info: 'info', fixed: 'ok' };
const PLACEHOLDER = {
  issue: "We're looking into connection problems in Europe.",
  info: 'A hotfix is coming tonight (NZ time).',
  fixed: 'Matches connect again. Thanks for your patience.',
};
const ENDS = [[null, 'Until ended'], [6, '6 hours'], [24, '1 day'], [72, '3 days'], [168, '1 week']];

const { data, error, reload } = useLoad(() => api('GET', '/notices'), () => null, { refresh: false });
watch(() => live.noticesTick, reload);
const list = computed(() => data.value?.notices || null);

const kind = ref('issue');
const text = ref('');
const hours = ref(null);
const editing = ref(null);
const busy = ref(false);
const confirming = ref(null);

const problem = computed(() => {
  const t = text.value.trim();
  if (!t) return 'Write what players should know.';
  if (t.length > MAX) return `${MAX} characters at most.`;
  if (!editing.value && (list.value || []).filter(n => n.live).length >= 3) return 'Three are live: end one first.';
  return '';
});

function reset() {
  editing.value = null;
  text.value = '';
  hours.value = null;
  kind.value = 'issue';
}
function edit(n) {
  editing.value = n.id;
  kind.value = n.kind;
  text.value = n.text;
  hours.value = n.ends ? Math.max(1, Math.round((n.ends - Date.now() / 1000) / 3600)) : null;
  document.getElementById('nt-text')?.focus();
}
async function save() {
  if (problem.value || busy.value) return;
  busy.value = true;
  const body = { text: text.value.trim(), kind: kind.value, hours: hours.value };
  try {
    if (editing.value) {
      await api('PUT', `/notices/${encodeURIComponent(editing.value)}`, body);
      toast('Notice changed: launchers show it again within a minute.');
    } else {
      await api('POST', '/notices', body);
      toast('Posted: every launcher shows it within a minute.');
    }
    reset();
    reload();
  } catch (e) {
    toast(e.status === 404 && !editing.value ? "This coordinator doesn't take notices yet." : e.message, true);
  } finally {
    busy.value = false;
  }
}
async function end(n) {
  busy.value = true;
  try {
    await api('DELETE', `/notices/${encodeURIComponent(n.id)}`);
    toast('Ended: launchers drop it within a minute.');
    confirming.value = null;
    if (editing.value === n.id) reset();
    reload();
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}
const when = t => new Date(t * 1000).toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' });
</script>

<style scoped>
.lead-in { margin: -4px 0 16px; max-width: 80ch; }
.post { display: grid; grid-template-columns: 150px minmax(0, 1fr); gap: 14px 18px; align-items: start; margin-bottom: 18px; }
.post > .label { padding-top: 9px; }
.text { max-width: 640px; }
.preview { background: var(--panel-2); border: 1px solid var(--line); border-left: 3px solid var(--info, var(--line)); border-radius: 8px; padding: 10px 12px; color: var(--text); }
.preview.issue { border-left-color: var(--warn); }
.preview.fixed { border-left-color: var(--ok); }
.actions { grid-column: 1 / -1; }
.warn-text { color: var(--warn); }
td .sub { display: block; color: var(--faint); font: 12px var(--mono); }
.confirm { gap: 6px; flex-wrap: nowrap; }
@media (max-width: 760px) {
  .post { grid-template-columns: minmax(0, 1fr); }
  .post > .label { padding-top: 0; }
}
</style>
