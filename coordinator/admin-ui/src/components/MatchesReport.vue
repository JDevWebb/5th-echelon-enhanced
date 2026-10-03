<template>
  <div v-if="!r" class="empty">{{ error || 'Loading…' }}</div>
  <template v-else>
    <div class="kpis">
      <div class="kpi"><span class="label">Matches</span><span class="v">{{ fmt.n(r.totals.matches) }}</span><span class="d">finished in this period</span></div>
      <div class="kpi"><span class="label">Length</span><span class="v">{{ r.totals.matches ? fmt.dur(r.totals.avg_seconds) : '–' }}</span><span class="d">on average</span></div>
      <div class="kpi"><span class="label">Players</span><span class="v">{{ r.totals.matches ? fmt.n(r.totals.avg_players, 1) : '–' }}</span><span class="d">per match, most at once</span></div>
      <div class="kpi"><span class="label">Private</span><span class="v">{{ r.totals.matches ? fmt.pct(r.private.matches / r.totals.matches * 100) : '–' }}</span><span class="d">{{ fmt.n(r.private.matches) }} private · {{ fmt.n(r.public.matches) }} public</span></div>
      <div class="kpi"><span class="label">Failed joins</span><span class="v">{{ fmt.n(failedJoins) }}</span><span class="d">players who couldn't join a match</span></div>
    </div>
    <section class="panel">
      <header><h2>Matches per day</h2><div class="legend"><span v-for="s in daySeries" :key="s.name"><i :style="{ background: s.color }"></i>{{ s.name }}</span></div></header>
      <LineChart :series="daySeries" :from="r.from" :to="r.days.at(-1).t" />
      <p class="small muted" style="margin-top: 8px">Finished matches by mode; "started" counts every match that began (from the servers' counters), finished or not.</p>
    </section>
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>By mode</h2></header>
        <div v-if="r.by_mode.length" class="table-wrap">
          <table>
            <thead><tr><th>Mode</th><th class="r">Matches</th><th class="r">Length</th><th class="r">Players</th></tr></thead>
            <tbody>
              <tr v-for="m in r.by_mode" :key="m.mode">
                <td>{{ modeName(m.mode) }}</td>
                <td class="r">{{ fmt.n(m.matches) }}</td>
                <td class="r">{{ fmt.dur(m.avg_seconds) }}</td>
                <td class="r">{{ fmt.n(m.avg_players, 1) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div v-else class="empty">No matches in this period.</div>
      </section>
      <section class="panel">
        <header><h2>Game modes</h2></header>
        <div v-if="r.game_modes.length" class="table-wrap">
          <table>
            <thead><tr><th>Game mode</th><th class="r">Matches</th><th class="r">Length</th><th class="r">Players</th></tr></thead>
            <tbody>
              <tr v-for="g in r.game_modes" :key="g.mode + g.game_mode">
                <td><Labelled kind="game_mode" :id="g.game_mode" :name="label('game_mode', g.game_mode)" @named="reload" /> <span class="small muted">{{ modeName(g.mode) }}</span></td>
                <td class="r">{{ fmt.n(g.matches) }}</td>
                <td class="r">{{ fmt.dur(g.avg_seconds) }}</td>
                <td class="r">{{ fmt.n(g.avg_players, 1) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div v-else class="empty">No matches in this period.</div>
      </section>
    </div>
    <section class="panel">
      <header><h2>Top maps</h2><span class="small muted">Name maps as you identify them; the names apply everywhere.</span></header>
      <div v-if="r.maps.length" class="table-wrap">
        <table>
          <thead><tr><th>Map</th><th>Mode</th><th class="r">Matches</th><th class="r">Length</th><th class="r">Players</th><th>Share</th></tr></thead>
          <tbody>
            <tr v-for="m in r.maps" :key="m.mode + m.map">
              <td><Labelled kind="map" :id="m.map" :name="label('map', m.map)" @named="reload" /></td>
              <td class="muted">{{ modeName(m.mode) }}</td>
              <td class="r">{{ fmt.n(m.matches) }}</td>
              <td class="r">{{ fmt.dur(m.avg_seconds) }}</td>
              <td class="r">{{ fmt.n(m.avg_players, 1) }}</td>
              <td style="min-width: 120px"><Bar label="" :frac="m.matches / r.totals.matches" :text="fmt.pct(m.matches / r.totals.matches * 100)" /></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No matches in this period. Servers on older releases don't report them.</div>
    </section>
  </template>
</template>

<script setup>
// What was played: finished matches per day by mode, how long they last, how many play,
// and on which maps (GET /matches-report).
import { computed, h } from 'vue';
import Bar from './Bar.vue';
import LineChart from './LineChart.vue';
import NameIt from './NameIt.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt, PALETTE } from '../lib/fmt.js';
import { openModal } from '../lib/ui.js';

const props = defineProps({ days: Number });
const { data: r, error, reload } = useLoad(() => api('GET', `/matches-report?days=${props.days}`), () => props.days);
const modeName = m => ({ svm: 'Spies vs Mercs', coop: 'Co-op' })[m] || 'Other';
const label = (kind, id) => r.value?.labels.find(x => x.kind === kind && x.id === id)?.name;
const failedJoins = computed(() => r.value.days.reduce((n, d) => n + d.failed_joins, 0));
const daySeries = computed(() => [
  { name: 'Spies vs Mercs', color: PALETTE[0], points: r.value.days.map(d => ({ t: d.t, v: d.svm })) },
  { name: 'Co-op', color: PALETTE[1], points: r.value.days.map(d => ({ t: d.t, v: d.coop })) },
  { name: 'Started', color: PALETTE[2], points: r.value.days.map(d => ({ t: d.t, v: d.started })) },
]);

/** A map or game mode by its name, or its number, with a button to name it. */
const Labelled = (p, { emit }) => h('span', { class: 'row' }, [
  p.name ? h('span', p.name) : h('span', { class: 'muted' }, `#${p.id}`),
  h('button', {
    class: 'ghost small', type: 'button', title: 'Name it',
    onClick: async () => { if (await openModal(NameIt, { kind: p.kind, id: p.id, current: p.name })) emit('named'); },
  }, p.name ? 'Rename' : 'Name'),
]);
Labelled.props = ['kind', 'id', 'name'];
Labelled.emits = ['named'];
</script>
