<template>
  <div class="map">
    <svg viewBox="0 6 360 152" role="img" :aria-label="`Map of ${unit} by city`">
      <line v-for="lon in lons" :key="'lo' + lon" class="graticule" :x1="lon + 180" :x2="lon + 180" y1="0" y2="180" />
      <line v-for="lat in lats" :key="'la' + lat" class="graticule" x1="0" x2="360" :y1="90 - lat" :y2="90 - lat" />
      <path class="land" :d="LAND" />
      <g v-for="p in spots" :key="p.key">
        <circle class="spot" :cx="p.cx" :cy="p.cy" :r="p.r"><title>{{ p.label }}</title></circle>
        <circle class="spot core" :cx="p.cx" :cy="p.cy" r="0.7" />
      </g>
      <rect v-for="s in serverSpots" :key="s.id" class="server" :x="s.cx - 1.6" :y="s.cy - 1.6" width="3.2" height="3.2" :transform="`rotate(45 ${s.cx} ${s.cy})`"><title>{{ s.name }} (server)</title></rect>
    </svg>
  </div>
</template>

<script setup>
import { computed } from 'vue';
import { LAND } from '../lib/world.js';
import { fmt, serverName } from '../lib/fmt.js';

const props = defineProps({ places: { type: Array, default: () => [] }, servers: { type: Array, default: () => [] }, unit: { type: String, default: 'players' } });
const lons = [-150, -120, -90, -60, -30, 0, 30, 60, 90, 120, 150];
const lats = [-60, -30, 0, 30, 60];
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
</script>
