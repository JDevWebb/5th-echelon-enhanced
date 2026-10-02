<template>
  <template v-if="o">
    <PageTop title="Servers" sub="Every member of the network, as each last reported." />
    <div v-if="o.servers.length" class="servers">
      <section v-for="sv in o.servers" :key="sv.id" class="panel server">
        <header>
          <div>
            <h2><span class="dot" :class="serverStatus(sv)[0]" :title="serverStatus(sv)[1]"></span><RouterLink :to="`/servers/${encodeURIComponent(sv.id)}`">{{ serverName(sv) }}</RouterLink></h2>
            <p class="meta">{{ [sv.listing?.region, sv.listing?.host].filter(Boolean).join(' · ') }}</p>
          </div>
          <span class="pill" :class="updatePill(sv, o.rollout)[0]">{{ updatePill(sv, o.rollout)[1] }}</span>
        </header>
        <p v-if="sv.delisted" class="callout warn">Delisted: {{ sv.delisted }}</p>
        <p v-if="!sv.online" class="callout bad">Offline since {{ fmt.when(sv.last_seen) }}</p>
        <p v-for="clash in sv.name_clashes || []" :key="clash" class="callout warn">Name: {{ clash }}</p>
        <div class="bars">
          <Bar label="CPU" :frac="(sys(sv).cpu_percent || 0) / 100" :text="fmt.pct(sys(sv).cpu_percent)" />
          <Bar label="Memory" :frac="sys(sv).mem_total ? (sys(sv).mem_total - sys(sv).mem_available) / sys(sv).mem_total : 0" :text="sys(sv).mem_total ? fmt.bytes(sys(sv).mem_total - sys(sv).mem_available) : '–'" />
          <Bar label="Disk" :frac="sys(sv).disk_total ? 1 - sys(sv).disk_free / sys(sv).disk_total : 0" :text="sys(sv).disk_total ? `${fmt.bytes(sys(sv).disk_free)} free` : '–'" />
        </div>
        <div class="facts">
          <div class="fact"><b>{{ fmt.n(sv.metrics?.players?.online) }}</b><span>players online</span></div>
          <div class="fact"><b>{{ fmt.rate(last(sv.id).rx) }}</b><span>in</span></div>
          <div class="fact"><b>{{ fmt.rate(last(sv.id).tx) }}</b><span>out</span></div>
          <div class="fact"><b>{{ fmt.ms(sv.ping_ms) }}</b><span>ping from coordinator</span></div>
          <div class="fact"><b>{{ fmt.dur(sv.metrics?.uptime_secs) }}</b><span>server uptime</span></div>
          <div class="fact"><b>{{ sys(sv).load ? sys(sv).load[0].toFixed(2) : '–' }}</b><span>load · {{ sys(sv).cpus || '?' }} CPUs</span></div>
        </div>
      </section>
    </div>
    <div v-else class="panel empty">No servers have joined yet. Install one with install-server.sh and the join token.</div>
  </template>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { computed, onMounted } from 'vue';
import PageTop from '../components/PageTop.vue';
import Bar from '../components/Bar.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName, serverStatus, updatePill } from '../lib/fmt.js';
import { live } from '../lib/live.js';

onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const sys = sv => sv.metrics?.system || {};
const { data: hour } = useLoad(() => api('GET', '/series?range=3600'));
const last = id => (hour.value?.points?.[id] || []).at(-1) || {};
</script>
