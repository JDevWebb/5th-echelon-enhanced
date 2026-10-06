<template>
  <PageTop :title="title" :sub="sub">
    <template v-if="stays.length" #stats>
      <Stat :value="fmt.n(people.length)" label="Players" sub="in it at some point" />
      <Stat :value="dur(span.to - span.from) || '<1 s'" label="Lasted" :sub="stillOpen ? 'still going' : 'from first to last'" />
      <Stat :value="fmt.n(trouble)" label="Trouble" :tone="trouble ? 'warn' : ''" sub="removals, drops and restarts" />
    </template>
    <RouterLink :to="'/sessions'" class="small">← Sessions</RouterLink>
  </PageTop>
  <div v-if="error && !data" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else-if="!data" class="empty">Loading…</div>
  <div v-else-if="!stays.length" class="panel"><div class="empty">Nothing on record for this match (events are kept 30 days).</div></div>
  <section v-else class="panel">
    <header><h2>What happened</h2><span class="small muted">every player's side, in order</span></header>
    <div class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Who</th><th>What</th></tr></thead>
        <tbody>
          <tr v-for="(row, i) in rows" :key="i">
            <td class="small muted nowrap">{{ time(row.at) }}</td>
            <td class="nowrap"><b>{{ row.name }}</b></td>
            <td><span class="dot" :class="row.cls"></span> {{ row.text }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<script setup>
import { computed } from 'vue';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import { api } from '../lib/api.js';
import { useLoad } from '../lib/data.js';
import { fmt } from '../lib/fmt.js';
import { dur, endClass, endText, markClass, netText, roomKind, stayTitle, where } from '../lib/stays.js';

const props = defineProps({ server: { type: String, required: true }, room: { type: String, required: true }, since: { type: String, default: '0' } });
const room = computed(() => Number(props.room));
const since = computed(() => Number(props.since) || 0);

// A match's events lie within its life: from when it was made (a day back when not known)
// to now, at most a week.
const { data, error } = useLoad(() => {
  const to = Math.floor(Date.now() / 1000);
  const from = since.value ? Math.max(since.value - 60, to - 7 * 86400) : to - 86400;
  return api('GET', `/sessions?${new URLSearchParams({ server: props.server, from: String(from), to: String(to) })}`);
}, () => [props.server, props.room, props.since]);

const inRoom = r => r.room === room.value && (!since.value || !r.since || r.since === since.value);
const stays = computed(() => (data.value?.players || []).flatMap(p => p.rooms.filter(inRoom).map(r => ({ ...r, name: p.name, player: p }))));
const people = computed(() => [...new Set(stays.value.map(s => s.name))]);
const span = computed(() => ({ from: Math.min(...stays.value.map(s => s.from)), to: Math.max(...stays.value.map(s => s.to)) }));
const stillOpen = computed(() => stays.value.some(s => s.end?.how === 'still'));
const trouble = computed(() => stays.value.filter(s => ['removed', 'dropped', 'restarted'].includes(s.end?.how)).length);
const first = computed(() => [...stays.value].sort((a, b) => a.from - b.from)[0]);
const title = computed(() => {
  const r = first.value;
  if (!r) return 'A match';
  return r.kind === 'match' ? where(data.value?.labels, r) || 'A match' : 'A party';
});
const sub = computed(() => {
  const r = first.value;
  if (!r) return '';
  const host = r.host ? r.name : r.host_name;
  return `${roomKind(r).replace(/^./, c => c.toUpperCase())}${host ? `, hosted by ${host}` : ''} · room ${props.room} on ${props.server}`;
});
const time = t => new Date(t * 1000).toLocaleString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });

// Every player's joins and leaves, and what they went through meanwhile.
const rows = computed(() => {
  const labels = data.value?.labels;
  const out = [];
  for (const s of stays.value) {
    out.push({ at: s.from, name: s.name, text: `${stayTitle(labels, s, { map: false })}${netText(s.net) ? ' · ' + netText(s.net) : ''}`, cls: 'ok' });
    if (s.end?.how !== 'still') out.push({ at: s.to, name: s.name, text: endText(s), cls: endClass(s) });
    for (const m of s.player.marks) {
      if (m.at >= s.from - 5 && m.at <= s.to + 60 && m.kind !== 'search') out.push({ at: m.at, name: s.name, text: m.text, cls: markClass(m) });
    }
  }
  const seen = new Set();
  return out
    .sort((a, b) => a.at - b.at)
    .filter(r => {
      const k = `${r.at}|${r.name}|${r.text}`;
      return !seen.has(k) && seen.add(k);
    });
});
</script>

<style scoped>
.nowrap { white-space: nowrap; }
</style>
