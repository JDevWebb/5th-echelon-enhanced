<template>
  <div class="table-wrap">
    <table>
      <thead><tr><th>Server</th><th>Region</th><th>Version</th><th class="r">Players</th><th class="r">CPU</th><th class="r">Memory</th><th class="r">Ping</th><th>Seen</th></tr></thead>
      <tbody>
        <tr v-for="sv in overview.servers" :key="sv.id">
          <td><RouterLink :to="`/servers/${encodeURIComponent(sv.id)}`"><span class="row"><span class="dot" :class="serverStatus(sv)[0]" :title="serverStatus(sv)[1]"></span>{{ serverName(sv) }}</span></RouterLink></td>
          <td class="muted">{{ sv.listing?.region || '–' }}</td>
          <td><span class="pill" :class="updatePill(sv, overview.rollout)[0]">{{ updatePill(sv, overview.rollout)[1] }}</span></td>
          <td class="r">{{ fmt.n(sv.metrics?.players?.online) }}</td>
          <td class="r">{{ fmt.pct(sv.metrics?.system?.cpu_percent) }}</td>
          <td class="r">{{ mem(sv) }}</td>
          <td class="r">{{ fmt.ms(sv.ping_ms) }}</td>
          <td class="muted">{{ fmt.ago(sv.last_seen) }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<script setup>
import { fmt, serverName, serverStatus, updatePill } from '../lib/fmt.js';
defineProps({ overview: Object });
const mem = sv => {
  const s = sv.metrics?.system || {};
  return s.mem_total ? fmt.pct((s.mem_total - s.mem_available) / s.mem_total * 100) : '–';
};
</script>
