<template>
  <PageTop title="Playlists" :sub="now ? 'What\'s being played right now, by mode and map.' : 'What was played, by mode and map (player-minutes).'">
    <RangePicker v-model="range" :options="PLACE_RANGES" />
  </PageTop>
  <section class="panel">
    <div v-if="!data" class="empty">Loading…</div>
    <div v-else-if="data.activity.length" class="table-wrap">
      <table>
        <thead><tr><th>Mode</th><th>Room</th><th>Map</th><th>Game mode</th><th class="r">{{ now ? 'Players' : 'Player-min' }}</th><th class="r">{{ now ? 'Sessions' : 'Session-min' }}</th><th>Share</th></tr></thead>
        <tbody>
          <tr v-for="(a, i) in data.activity" :key="i">
            <td>{{ a.mode === 'svm' ? 'Spies vs Mercs' : a.mode === 'coop' ? 'Co-op' : 'Any' }}</td>
            <td class="muted">{{ a.room === 'match' ? 'Match' : 'Lobby' }}</td>
            <td v-for="kind in ['map', 'game_mode']" :key="kind">
              <span v-if="a[kind] == null" class="faint">–</span>
              <span v-else class="row">
                <span v-if="label(kind, a[kind])">{{ label(kind, a[kind]) }}</span><span v-else class="muted">#{{ a[kind] }}</span>
                <button class="ghost small" type="button" title="Name it" @click="nameIt(kind, a[kind])">{{ label(kind, a[kind]) ? 'Rename' : 'Name' }}</button>
              </span>
            </td>
            <td class="r">{{ fmt.n(a.players) }}</td>
            <td class="r">{{ fmt.n(a.sessions) }}</td>
            <td style="min-width: 120px"><Bar label="" :frac="total ? a.players / total : 0" :text="fmt.pct(total ? a.players / total * 100 : 0)" /></td>
          </tr>
        </tbody>
      </table>
    </div>
    <div v-else class="empty">{{ now ? 'No lobbies or matches right now.' : 'Nothing was played in this period.' }}</div>
    <p class="small muted" style="margin-top: 10px">The game reports maps and modes by number. Name them as you identify them; the names apply everywhere.</p>
  </section>
</template>

<script setup>
import { computed, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import Bar from '../components/Bar.vue';
import NameIt from '../components/NameIt.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt, PLACE_RANGES } from '../lib/fmt.js';
import { openModal } from '../lib/ui.js';

const range = ref(0);
const now = computed(() => range.value === 0);
const { data, reload } = useLoad(() => api('GET', `/activity?range=${range.value}`), () => range.value);
const total = computed(() => (data.value?.activity || []).reduce((n, a) => n + a.players, 0));
const label = (kind, id) => data.value?.labels.find(x => x.kind === kind && x.id === id)?.name;
async function nameIt(kind, id) {
  if (await openModal(NameIt, { kind, id, current: label(kind, id) })) reload();
}
</script>
