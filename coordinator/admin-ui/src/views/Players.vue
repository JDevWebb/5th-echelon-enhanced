<template>
  <PageTop title="Players & map" :sub="range === 0 ? 'Players online now, by city.' : 'Time played, by city (player-minutes).'">
    <RangePicker v-model="range" :options="PLACE_RANGES" />
  </PageTop>
  <template v-if="data">
    <section class="panel">
      <WorldMap :places="data.places" :servers="o?.servers || []" :unit="data.unit" />
      <div class="row" style="justify-content: space-between; margin-top: 8px">
        <span class="legend"><span><i style="background: var(--accent)"></i>{{ data.unit === 'players' ? 'players' : 'player-minutes' }}</span><span><i style="background: var(--warn)"></i>servers</span></span>
        <span class="attr">{{ data.attribution }}</span>
      </div>
    </section>
    <section class="panel">
      <header><h2>By city</h2><span class="muted small">{{ fmt.n(total) }} {{ data.unit }} · {{ data.places.length }} places</span></header>
      <div v-if="data.places.length" class="table-wrap">
        <table>
          <thead><tr><th>City</th><th>Region</th><th>Country</th><th class="r">{{ data.unit === 'players' ? 'Players' : 'Minutes' }}</th><th class="r">Share</th><th>Servers</th></tr></thead>
          <tbody>
            <tr v-for="(p, i) in data.places" :key="i">
              <td>{{ p.city || (p.country ? '–' : 'Unknown or private network') }}</td>
              <td class="muted">{{ p.region || '–' }}</td>
              <td>{{ p.country ? `${flag(p.country)} ${p.country_name || p.country}` : '–' }}</td>
              <td class="r">{{ fmt.n(p.amount) }}</td>
              <td class="r">{{ fmt.pct(total ? p.amount / total * 100 : 0) }}</td>
              <td class="muted small">{{ Object.entries(p.servers || {}).sort((a, b) => b[1] - a[1]).map(([sid, n]) => `${names.get(sid) || sid} ${fmt.n(n)}`).join(' · ') }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">{{ range === 0 ? 'Nobody is playing right now.' : 'No players in this period.' }}</div>
    </section>
  </template>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import RangePicker from '../components/RangePicker.vue';
import WorldMap from '../components/WorldMap.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { flag, fmt, PLACE_RANGES, serverName } from '../lib/fmt.js';
import { live } from '../lib/live.js';

const range = ref(0);
onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const names = computed(() => new Map((o.value?.servers || []).map(sv => [sv.id, serverName(sv)])));
const { data } = useLoad(() => api('GET', `/places?range=${range.value}`), () => range.value);
const total = computed(() => (data.value?.places || []).reduce((n, p) => n + p.amount, 0));
</script>
