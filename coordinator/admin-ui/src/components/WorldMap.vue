<template>
  <div ref="box" class="map" @mouseleave="hover = null">
    <svg viewBox="0 6 360 152" role="img" :aria-label="`Map of ${live ? 'players online' : unit} by city`">
      <line v-for="lon in lons" :key="'lo' + lon" class="graticule" :x1="lon + 180" :x2="lon + 180" y1="0" y2="180" />
      <line v-for="lat in lats" :key="'la' + lat" class="graticule" x1="0" x2="360" :y1="90 - lat" :y2="90 - lat" />
      <path class="land" :d="LAND" />
      <!-- Night, deepening through the twilights. -->
      <path v-for="(d, i) in night" :key="'n' + i" class="night" :d="d" />
      <!-- Drawn again across the edge when it's near one. -->
      <g v-for="x in sunXs" :key="'s' + x" class="sun" :transform="`translate(${x} ${90 - sun.lat})`">
        <circle r="2.2" />
        <title>The sun is overhead here ({{ utc }} UTC)</title>
      </g>
      <template v-if="!live">
        <g v-for="p in spots" :key="p.key">
          <circle class="spot" :cx="p.cx" :cy="p.cy" :r="p.r"><title>{{ p.label }}</title></circle>
          <circle class="spot core" :cx="p.cx" :cy="p.cy" r="0.7" />
        </g>
      </template>
      <rect v-for="s in serverSpots" :key="s.id" class="server" :x="s.cx - 1.6" :y="s.cy - 1.6" width="3.2" height="3.2" :transform="`rotate(45 ${s.cx} ${s.cy})`"><title>{{ s.name }} (server)</title></rect>
      <g v-for="g in groups" :key="g.key" class="player" :class="g.status" tabindex="0" @mouseenter="show(g, $event)" @focus="show(g, $event)" @blur="hover = null">
        <circle class="ring" :cx="g.cx" :cy="g.cy" :r="1.6 + Math.min(4, Math.sqrt(g.players.length) * 0.9)" />
        <circle class="dot" :cx="g.cx" :cy="g.cy" r="1" />
      </g>
    </svg>
    <div v-if="hover" class="tip" :style="tipStyle" role="tooltip">
      <div class="tip-place">
        <span>{{ flag(hover.country) }} {{ hover.place }}</span>
        <span class="muted" :title="hover.exact ? `The clocks there (${hover.zone})` : 'Sun time, from the longitude: within an hour or so of the clocks there'">{{ hover.exact ? '' : 'about ' }}{{ hover.time }} there<template v-if="hover.exact"> ({{ hover.zone }})</template> · {{ hover.daylight }}</span>
      </div>
      <div v-for="p in hover.players.slice(0, 8)" :key="p.server + p.id" class="tip-player">
        <div class="row" style="justify-content: space-between; gap: 8px">
          <b>{{ p.name }}</b>
          <span class="pill" :class="p.status === 'match' ? 'ok' : ''">{{ statusLabel(p) }}</span>
        </div>
        <div class="small muted">
          {{ serverNames.get(p.server) || p.server }}<template v-if="p.since"> · online {{ fmt.dur(now - p.since) || '<1m' }}</template><template v-if="p.network"> · {{ NETWORK[p.network] }}</template>
        </div>
        <div v-if="p.with?.length" class="small muted">with {{ p.with.join(', ') }}</div>
      </div>
      <div v-if="hover.players.length > 8" class="small muted">and {{ hover.players.length - 8 }} more</div>
    </div>
    <p v-if="live && unplaced" class="small muted unplaced">{{ unplaced }} online without a known location</p>
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { api } from '../lib/api.js';
import { LAND } from '../lib/world.js';
import { flag, fmt, serverName } from '../lib/fmt.js';
import { darkPath, elevation, localTime, subsolar } from '../lib/sun.js';

const props = defineProps({
  places: { type: Array, default: () => [] },
  servers: { type: Array, default: () => [] },
  unit: { type: String, default: 'players' },
  /** Show each player online now (fetched here, every ten seconds) instead of city counts. */
  live: { type: Boolean, default: false },
});
const lons = [-150, -120, -90, -60, -30, 0, 30, 60, 90, 120, 150];
const lats = [-60, -30, 0, 30, 60];
const NETWORK = { relayed: 'relayed', direct: 'direct', unregistered: 'not registered for online play' };
const MODES = { coop: 'co-op', svm: 'Spies vs Mercs' };

// The time, for the sun and the tooltips: every half minute is plenty.
const now = ref(Date.now() / 1000);
const sun = computed(() => subsolar(new Date(now.value * 1000)));
const night = computed(() => [0, -6, -12, -18].map(below => darkPath(sun.value, below)));
const sunXs = computed(() => {
  const x = sun.value.lon + 180;
  return x < 4 ? [x, x + 360] : x > 356 ? [x, x - 360] : [x];
});
const utc = computed(() => new Date(now.value * 1000).toISOString().slice(11, 16));

const online = ref([]);
let timers = [];
async function fetchOnline() {
  if (!props.live) return;
  try { online.value = (await api('GET', '/online')).players || []; } catch { /* the map stays as it was */ }
}
onMounted(() => {
  fetchOnline();
  timers = [setInterval(fetchOnline, 10000), setInterval(() => { now.value = Date.now() / 1000; }, 30000)];
});
onUnmounted(() => timers.forEach(clearInterval));

const spots = computed(() => {
  const located = props.places.filter(p => p.country && (p.lat || p.lon));
  const top = Math.max(1, ...located.map(p => p.amount));
  return [...located].sort((a, b) => b.amount - a.amount).map((p, i) => ({
    key: `${p.city}-${p.country}-${i}`,
    cx: p.lon + 180,
    cy: 90 - p.lat,
    r: 1.1 + Math.sqrt(p.amount / top) * 5.5,
    label: `${[p.city, p.region, p.country_name || p.country].filter(Boolean).join(', ')}: ${fmt.n(p.amount)} ${props.unit}`,
  }));
});
const serverSpots = computed(() => props.servers.filter(sv => sv.place).map(sv => ({ id: sv.id, name: serverName(sv), cx: sv.place.lon + 180, cy: 90 - sv.place.lat })));
const serverNames = computed(() => new Map(props.servers.map(sv => [sv.id, serverName(sv)])));

// Players in one city share a marker; it shows what the busiest of them is doing.
const RANK = { match: 0, lobby: 1, menus: 2 };
const groups = computed(() => {
  if (!props.live) return [];
  const by = new Map();
  for (const p of online.value) {
    if (!p.country || !(p.lat || p.lon)) continue;
    const key = `${p.lat.toFixed(2)},${p.lon.toFixed(2)}`;
    if (!by.has(key)) by.set(key, { key, cx: p.lon + 180, cy: 90 - p.lat, lat: p.lat, lon: p.lon, country: p.country, place: [p.city, p.region, p.country_name || p.country].filter(Boolean).join(', '), players: [] });
    by.get(key).players.push(p);
  }
  return [...by.values()].map(g => {
    g.players.sort((a, b) => RANK[a.status] - RANK[b.status] || a.name.localeCompare(b.name));
    g.status = g.players[0].status;
    return g;
  });
});
const unplaced = computed(() => props.live ? online.value.filter(p => !p.country || !(p.lat || p.lon)).length : 0);

function statusLabel(p) {
  if (p.status === 'match') return MODES[p.mode] ? `In a ${MODES[p.mode]} match` : 'In a match';
  if (p.status === 'lobby') return 'In a lobby';
  return 'In the menus';
}

// The tooltip, beside the marker, kept inside the map.
const box = ref(null);
const hover = ref(null);
const tipAt = ref({ x: 0, y: 0, right: false });
function show(g, event) {
  const rect = box.value?.getBoundingClientRect();
  const target = event.currentTarget.getBoundingClientRect();
  if (!rect) return;
  const x = target.left + target.width / 2 - rect.left;
  const y = target.top + target.height / 2 - rect.top;
  const e = elevation(g.lat, g.lon, sun.value);
  const clock = localTime(g.country, g.lat, g.lon, new Date(now.value * 1000));
  hover.value = {
    ...g,
    time: clock.time,
    zone: clock.zone,
    exact: clock.exact,
    daylight: e > 0 ? 'day' : e > -6 ? 'twilight' : 'night',
  };
  tipAt.value = { x, y, right: x > rect.width / 2 };
}
const tipStyle = computed(() => (tipAt.value.right
  ? { right: `${(box.value?.clientWidth || 0) - tipAt.value.x + 12}px`, top: `${Math.max(0, tipAt.value.y - 20)}px` }
  : { left: `${tipAt.value.x + 12}px`, top: `${Math.max(0, tipAt.value.y - 20)}px` }));
</script>

<style scoped>
/* The theme's night (styles.css), a layer per twilight: deepest where all four overlap. */
.night { fill: var(--night); fill-opacity: var(--night-alpha); pointer-events: none; }
.sun circle { fill: #ffd166; stroke: rgba(255, 209, 102, 0.35); stroke-width: 1.6; }
.player { cursor: pointer; outline: none; }
.player .ring { fill: var(--info); fill-opacity: 0.22; stroke: var(--info); stroke-width: 0.3; }
.player .dot { fill: var(--info); }
.player.match .ring { fill: var(--accent); stroke: var(--accent); }
.player.match .dot { fill: var(--accent); }
.player.menus .ring { fill: var(--muted); stroke: var(--muted); fill-opacity: 0.18; }
.player.menus .dot { fill: var(--muted); }
.player:hover .ring, .player:focus .ring { fill-opacity: 0.45; stroke-width: 0.5; }
.tip {
  position: absolute; z-index: 5; min-width: 220px; max-width: 300px; pointer-events: none;
  background: var(--panel); border: 1px solid var(--line-2); border-radius: 10px; box-shadow: var(--shadow);
  padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; font-size: 13px;
}
.tip-place { display: flex; flex-direction: column; gap: 2px; padding-bottom: 6px; border-bottom: 1px solid var(--line); }
.tip-player { display: flex; flex-direction: column; gap: 2px; }
.unplaced { margin: 6px 0 0; }
</style>
