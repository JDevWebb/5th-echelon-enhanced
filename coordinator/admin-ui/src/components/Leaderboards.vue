<template>
  <section class="panel">
    <header>
      <h2>Leaderboards <span class="muted small">{{ list ? `${fmt.n(list.total)} ranked` : '' }}</span></h2>
      <form class="row filters" @submit.prevent>
        <select :value="board" aria-label="Leaderboard" @change="pickBoard(Number($event.target.value))">
          <option v-for="l in data?.leaderboards || []" :key="l.id" :value="l.id">{{ LABELS[l.id] || `Leaderboard ${l.id}` }}</option>
        </select>
        <select v-model.number="context" aria-label="Mission, mode or ladder">
          <option v-for="c in contexts" :key="c" :value="c">{{ contextName(board, c) }}{{ totals.get(`${board}/${c}`) ? ` · ${fmt.n(totals.get(`${board}/${c}`))}` : '' }}</option>
        </select>
      </form>
    </header>
    <p v-if="actionError" class="err">{{ actionError }}</p>
    <div v-if="!data" class="empty">{{ error || 'Loading…' }}</div>
    <div v-else-if="!list || !list.top.length" class="empty">{{ data.lists.length ? 'Nobody on this list yet.' : 'No stats yet. Servers send them as people play (newer releases only).' }}</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th class="r">Rank</th><th>Name</th><th class="r">{{ VALUE_NAMES[board] || 'Value' }}</th><th>Global id</th><th></th></tr></thead>
        <tbody>
          <tr v-for="p in list.top" :key="p.global_id">
            <td class="r">{{ p.rank }}</td>
            <td><b>{{ p.name || '–' }}</b></td>
            <td class="r">{{ value(board, p.value) }}</td>
            <td class="muted small mono" :title="p.global_id">{{ p.global_id.length > 16 ? `${p.global_id.slice(0, 16)}…` : p.global_id }}</td>
            <td class="r"><button class="ghost small danger" type="button" :disabled="removing === p.global_id" @click="remove(p)">Remove stats</button></td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="small muted" style="margin-top: 8px">Across the network: one entry per person (their identity), however many servers they play on. The top 100 of each list.</p>
  </section>
</template>

<script setup>
// The global leaderboards (GET /leaderboards), and removing a person's stats (a cheater's).
import { computed, ref, watch } from 'vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt } from '../lib/fmt.js';

const LABELS = {
  1: 'Solo high score', 2: 'Solo best time', 3: 'Co-op high score', 4: 'Co-op best time', 5: 'Spies vs Mercs total score',
  6: 'Ladder time played', 7: 'Ladder wins', 8: 'Ladder deaths from above', 9: 'Ladder takedowns', 10: 'Ladder kills',
  11: 'Ladder total score', 12: 'Ladder objectives',
};
const VALUE_NAMES = { 1: 'Score', 2: 'Time', 3: 'Score', 4: 'Time', 5: 'Score', 6: 'Played', 7: 'Wins', 8: 'Deaths from above', 9: 'Takedowns', 10: 'Kills', 11: 'Score', 12: 'Objectives' };
// Spies vs Mercs game modes, as far as they're known.
const MODES = { 227: 'TDM?', 228: 'Spies vs Mercs?', 229: 'Extraction?' };

function contextName(l, c) {
  if (l <= 4) return `Mission ${c}`;
  if (l === 5) return MODES[c] ? `Mode ${c} (${MODES[c]})` : `Mode ${c}`;
  return `Ladder ${c}`;
}

function value(l, v) {
  if (l === 2 || l === 4) {
    const s = Math.round(v);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }
  if (l === 6) return `${fmt.n(v / 3600, 1)} h`;
  return fmt.n(v);
}

const board = ref(null);
const context = ref(null);
const query = computed(() => (board.value && context.value != null ? `?leaderboard=${board.value}&context=${context.value}` : ''));
const { data, error, reload } = useLoad(() => api('GET', `/leaderboards${query.value}`), () => query.value, { refresh: false });
const totals = computed(() => new Map((data.value?.lists || []).map(x => [`${x.leaderboard}/${x.context}`, x.total])));
const contexts = computed(() => data.value?.leaderboards.find(l => l.id === board.value)?.contexts || []);
const list = computed(() => data.value?.list || null);

// At first, the first list with anyone on it; a new leaderboard opens its fullest context.
watch(data, d => {
  if (d && board.value == null) {
    const first = d.lists[0] || { leaderboard: d.leaderboards[0].id, context: d.leaderboards[0].contexts[0] };
    board.value = first.leaderboard;
    context.value = first.context;
  }
});
function pickBoard(l) {
  const ranked = data.value.lists.filter(x => x.leaderboard === l).sort((a, b) => b.total - a.total);
  context.value = ranked[0]?.context ?? data.value.leaderboards.find(x => x.id === l)?.contexts[0];
  board.value = l;
}

const removing = ref('');
const actionError = ref('');
async function remove(p) {
  const who = p.name || p.global_id;
  if (!await confirmBox(`Remove ${who}'s stats?`, `Everything the network keeps for ${who} goes, on every leaderboard. It can't be undone. Their servers keep their own copy, and games they play from now on count again.`, 'Remove stats', true)) return;
  removing.value = p.global_id;
  actionError.value = '';
  try {
    await api('DELETE', `/stats/${encodeURIComponent(p.global_id)}`);
    await reload();
  } catch (e) {
    actionError.value = e.message;
  } finally {
    removing.value = '';
  }
}
</script>

<style scoped>
.filters { flex-wrap: wrap; }
.filters select { width: auto; }
</style>
