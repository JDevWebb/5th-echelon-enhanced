<template>
  <PageTop title="Servers" sub="Every member of the network, as each last reported.">
    <template v-if="o" #stats>
      <Stat :value="fmt.n(online)" :unit="`/ ${o.servers.length}`" label="Online" :tone="online < o.servers.length ? 'warn' : ''" />
      <Stat :value="fmt.n(players)" label="Players online" />
      <Stat :value="fmt.n(delisted)" label="Delisted" :tone="delisted ? 'warn' : ''" />
      <Stat :value="o.rollout.target || '–'" label="Release" :sub="behind ? `${behind} behind` : 'all on it'" />
    </template>
    <RouterLink class="btn" to="/updates">Updates</RouterLink>
  </PageTop>
  <template v-if="o">
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
        <p v-if="sv.host_check && !sv.host_check.ok" class="callout warn">Not in the directory: the server at its host didn't answer as this one ({{ sv.host_check.why }})</p>
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
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { fmt, serverName, serverStatus, updatePill } from '../lib/fmt.js';
import { live } from '../lib/live.js';

onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const online = computed(() => o.value.servers.filter(sv => sv.online).length);
const players = computed(() => o.value.servers.reduce((n, sv) => n + (sv.online ? sv.metrics?.players?.online || 0 : 0), 0));
const delisted = computed(() => o.value.servers.filter(sv => sv.delisted).length);
const behind = computed(() => o.value.servers.filter(sv => o.value.rollout.target && sv.listing?.version && sv.listing.version !== o.value.rollout.target).length);
const sys = sv => sv.metrics?.system || {};
const { data: hour } = useLoad(() => api('GET', '/series?range=3600'));
const last = id => (hour.value?.points?.[id] || []).at(-1) || {};
</script>
