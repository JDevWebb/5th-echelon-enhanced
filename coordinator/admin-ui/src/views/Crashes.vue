<template>
  <PageTop title="Crashes" sub="Every player's crashes and freezes, sent by their game when it starts again, grouped by where they happened. A new one that hits three players in a day, or a fixed one seen again, is an alert.">
    <template v-if="data" #stats>
      <Stat :value="fmt.n(data.crashes)" label="Crashes and freezes" :sub="`last ${data.days} days`" />
      <Stat :value="fmt.n(data.fatal)" label="Ended the game" :tone="data.fatal ? 'warn' : ''" />
      <Stat :value="fmt.n(data.players)" label="Players hit" />
      <Stat :value="fmt.n(newOnes)" label="Not marked" :tone="newOnes ? 'warn' : 'ok'" />
    </template>
    <div class="seg" role="group" aria-label="Range">
      <button v-for="d in [1, 7, 30]" :key="d" type="button" :aria-pressed="String(days === d)" @click="days = d">{{ d === 1 ? '24 h' : `${d} d` }}</button>
    </div>
  </PageTop>
  <template v-if="data">
    <section class="panel">
      <header>
        <h2>By where</h2>
        <div class="seg" role="group" aria-label="Show">
          <button v-for="[id, label] in SHOW" :key="id" type="button" :aria-pressed="String(show === id)" @click="show = id">{{ label }}</button>
        </div>
      </header>
      <div v-if="!shown.length" class="empty">{{ data.groups.length ? 'Nothing here with this choice.' : 'No crashes in this period.' }}</div>
      <div v-else class="stack">
        <div v-for="g in shown" :key="g.signature" class="callout" :class="tone(g)">
          <div class="row" style="justify-content: space-between; align-items: baseline">
            <b class="mono">{{ g.signature }}</b>
            <span class="small muted">{{ fmt.n(g.players) }} players · {{ fmt.n(g.count) }} times · {{ fmt.n(g.fatal) }} ended the game · last {{ fmt.ago(g.last) }}</span>
          </div>
          <div class="row small" style="gap: 6px; flex-wrap: wrap">
            <span v-if="g.mark" class="pill" :class="g.mark.status === 'fixed' ? 'ok' : 'info'">{{ markText(g.mark) }}</span>
            <span v-else class="pill warn">not marked</span>
            <span v-if="g.seen_since_fix.length" class="pill bad">seen again on {{ g.seen_since_fix.join(', ') }}</span>
            <span class="muted">{{ g.kind === 'freeze' ? 'Freeze' : g.latest.code }} · releases {{ g.releases.join(', ') || '?' }} · {{ g.servers.join(', ') }}</span>
          </div>
          <div v-if="g.mark?.note" class="small muted">{{ g.mark.note }}</div>
          <details>
            <summary class="small">Details: {{ g.names.join(', ') }}{{ g.players > g.names.length ? ' …' : '' }}</summary>
            <div class="stack small" style="margin-top: 8px">
              <div><span class="muted">Latest</span> {{ fmt.when(g.latest.at) }}, {{ g.latest.name || 'someone' }}, {{ g.latest.release || '?' }}{{ g.latest.fatal ? ', ended the game' : ', carried on' }}</div>
              <div v-if="g.latest.doing"><span class="muted">Doing</span> {{ g.latest.doing }}</div>
              <div v-if="g.latest.registers"><span class="muted">Registers</span> <code class="wrap">{{ g.latest.registers }}</code></div>
              <div v-if="g.latest.callers"><span class="muted">Callers</span> <code class="wrap">{{ g.latest.callers }}</code></div>
              <div v-if="g.latest.stack"><span class="muted">On the stack</span> <code class="wrap">{{ g.latest.stack }}</code></div>
              <div v-if="g.reports.length"><span class="muted">Reports</span> <RouterLink v-for="id in g.reports" :key="id" :to="`/reports/${id}`" class="mono" style="margin-right: 8px">{{ id.slice(0, 6) }}</RouterLink></div>
              <form class="row" @submit.prevent="mark(g)">
                <select v-model="edit[g.signature].status" aria-label="Mark">
                  <option value="">Not marked</option>
                  <option value="fixed">Fixed in</option>
                  <option value="known">Known, nothing to do</option>
                  <option value="watching">Watching</option>
                </select>
                <input v-if="edit[g.signature].status === 'fixed'" v-model="edit[g.signature].fixed_in" placeholder="0.5.0" aria-label="Release" style="width: 90px">
                <input v-model="edit[g.signature].note" placeholder="Note (what it is)" aria-label="Note" style="flex: 1 1 240px">
                <button type="submit">Save</button>
              </form>
            </div>
          </details>
        </div>
      </div>
    </section>
    <section class="panel">
      <header><h2>Crash fixes in players' games</h2><span class="small muted">since {{ fmt.when(data.since) }}</span></header>
      <div v-if="data.fixes.length" class="table-wrap">
        <table>
          <thead><tr><th>Fix</th><th class="r">Kept a game going</th><th class="r">Players</th><th class="r">Runs</th><th>Didn't go in</th></tr></thead>
          <tbody>
            <tr v-for="f in data.fixes" :key="f.name">
              <td class="mono">{{ f.name }}</td>
              <td class="r">{{ fmt.n(f.saves) }}</td>
              <td class="r">{{ fmt.n(f.players) }}</td>
              <td class="r">{{ fmt.n(f.runs) }}</td>
              <td :class="Object.keys(f.missing).length ? 'warn' : 'muted'">{{ Object.entries(f.missing).map(([b, n]) => `${b} (${n})`).join(', ') || '–' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No counts yet: games from 0.5.0 on send them.</div>
    </section>
    <section v-if="data.marked_unseen.length" class="panel">
      <header><h2>Marked, not seen in this period</h2></header>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Where</th><th>Mark</th><th>Note</th></tr></thead>
          <tbody>
            <tr v-for="m in data.marked_unseen" :key="m.signature">
              <td class="mono">{{ m.signature }}</td>
              <td>{{ markText(m) }}</td>
              <td class="small muted">{{ m.note }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, reactive, ref, watch } from 'vue';
import { RouterLink } from 'vue-router';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt } from '../lib/fmt.js';
import { toast } from '../lib/ui.js';

const SHOW = [['open', 'To look at'], ['all', 'All'], ['fixed', 'Fixed']];
const days = ref(7);
const show = ref('open');
const { data, error, reload } = useLoad(() => api('GET', `/crashes?days=${days.value}`), () => days.value, { refresh: false });

// Each group's mark as edited, from what's saved.
const edit = reactive({});
watch(data, d => {
  for (const g of d?.groups || []) {
    edit[g.signature] = { status: g.mark?.status || '', fixed_in: g.mark?.fixed_in || '', note: g.mark?.note || '' };
  }
});

const newOnes = computed(() => (data.value?.groups || []).filter(g => !g.mark).length);
const shown = computed(() => (data.value?.groups || []).filter(g => {
  if (show.value === 'all') return true;
  if (show.value === 'fixed') return g.mark?.status === 'fixed';
  // To look at: not marked, watched, or back after a fix.
  return !g.mark || g.mark.status === 'watching' || g.seen_since_fix.length;
}));

function tone(g) {
  if (g.seen_since_fix.length) return 'bad';
  if (!g.mark) return g.fatal ? 'bad' : 'warn';
  return g.mark.status === 'watching' ? 'warn' : '';
}
function markText(m) {
  return m.status === 'fixed' ? `fixed in ${m.fixed_in}` : m.status === 'known' ? 'known, nothing to do' : 'watching';
}
async function mark(g) {
  const e = edit[g.signature];
  try {
    await api('PUT', '/crashes/mark', { signature: g.signature, status: e.status, fixed_in: e.fixed_in.trim(), note: e.note.trim() });
    toast(e.status ? 'Marked.' : 'Mark cleared.');
    reload();
  } catch (err) {
    toast(err.message, true);
  }
}
</script>

<style scoped>
code.wrap { white-space: pre-wrap; word-break: break-all; }
.callout:not(.bad):not(.warn) { border-color: var(--line); }
td.warn { color: var(--warn); }
</style>
