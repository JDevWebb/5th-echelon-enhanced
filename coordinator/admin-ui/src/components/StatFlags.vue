<template>
  <section class="panel">
    <header>
      <h2>Flagged stats <span class="muted small">{{ data ? `${fmt.n(data.flags.length)} in the last ${data.days} days` : '' }}</span></h2>
    </header>
    <p v-if="actionError" class="err">{{ actionError }}</p>
    <div v-if="!data" class="empty">{{ error || 'Loading…' }}</div>
    <div v-else-if="!data.flags.length" class="empty">Nothing flagged. A stat gain is flagged at over {{ data.times }}× what the network's players normally gain in a minute of play (once a stat has {{ data.min_samples }} gains to compare with).</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Name</th><th>Stat</th><th class="r">Gained</th><th class="r">Into the session</th><th class="r">× normal</th><th></th></tr></thead>
        <tbody>
          <tr v-for="f in data.flags" :key="f.id">
            <td class="small">{{ fmt.when(f.at) }}</td>
            <td><b>{{ f.name || '–' }}</b> <span class="muted small mono" :title="f.global_id">{{ f.global_id.slice(0, 10) }}…</span></td>
            <td class="small">board {{ f.board }} · stat {{ f.stat }}{{ f.context ? ` · ${f.context}` : '' }}</td>
            <td class="r">{{ fmt.n(Math.round(f.gain)) }}</td>
            <td class="r">{{ fmt.dur(f.played) }}</td>
            <td class="r"><span class="pill bad">{{ fmt.n(Math.round(f.times_normal)) }}×</span></td>
            <td class="r"><button class="ghost small danger" type="button" :disabled="removing === f.global_id" @click="remove(f)">Remove stats</button></td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="small muted" style="margin-top: 8px">What a stat write added, per minute the player had been playing, against that stat's 99th percentile across the network. Nothing is hidden or changed for players: check their Sessions and reports, then remove their stats or ban them if it's cheating.</p>
    <h3 style="margin-top: 18px">Records far from everyone else's <span class="muted small">{{ data ? fmt.n((data.records || []).length) : '' }}</span></h3>
    <div v-if="data && !(data.records || []).length" class="empty">None. A high score is flagged at over {{ data.record_times }}× the 90th percentile on its mission or mode, a best time at under a {{ data.record_times }}rd of the 10th, once {{ data.record_samples }} players have one there.</div>
    <div v-else-if="data" class="table-wrap">
      <table>
        <thead><tr><th>Name</th><th>Record</th><th class="r">Theirs</th><th class="r">Usual best</th><th class="r">× usual</th><th></th></tr></thead>
        <tbody>
          <tr v-for="r in data.records" :key="`${r.global_id}${r.board}${r.stat}${r.context}`">
            <td><b>{{ r.name || '–' }}</b> <span class="muted small mono" :title="r.global_id">{{ r.global_id.slice(0, 10) }}…</span></td>
            <td class="small">{{ r.kind === 'fast' ? 'best time' : 'high score' }} · board {{ r.board }} · stat {{ r.stat }} · {{ r.context }}</td>
            <td class="r">{{ fmt.n(Math.round(r.value)) }}</td>
            <td class="r">{{ fmt.n(Math.round(r.normal)) }}</td>
            <td class="r"><span class="pill bad">{{ r.times_normal.toFixed(1) }}×</span></td>
            <td class="r"><button class="ghost small danger" type="button" :disabled="removing === r.global_id" @click="remove(r)">Remove stats</button></td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<script setup>
// Stat gains far above normal (GET /stats/flags, fair play F1), and removing a person's stats.
import { ref } from 'vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt } from '../lib/fmt.js';

const { data, error, reload } = useLoad(() => api('GET', '/stats/flags'), () => '', { refresh: false });
const removing = ref('');
const actionError = ref('');
async function remove(f) {
  const who = f.name || f.global_id;
  if (!await confirmBox(`Remove ${who}'s stats?`, `Everything the network keeps for ${who} goes, on every leaderboard, and their flags with it. It can't be undone.`, 'Remove stats', true)) return;
  removing.value = f.global_id;
  actionError.value = '';
  try {
    await api('DELETE', `/stats/${encodeURIComponent(f.global_id)}`);
    await reload();
  } catch (e) {
    actionError.value = e.message;
  } finally {
    removing.value = '';
  }
}
</script>
