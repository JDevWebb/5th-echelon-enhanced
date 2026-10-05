<template>
  <PageTop title="Updates" :sub="u ? `Releases from github.com/${u.repo}, signed with the release key. Servers check the signature again before installing. Book maintenance below.` : 'Signed releases, the rollout, and maintenance windows.'">
    <template v-if="u && o" #stats>
      <Stat :value="r.target || '–'" label="Release" :sub="r.previous ? `previous ${r.previous}` : 'no previous one'" />
      <Stat :value="r.target ? (r.paused ? 'paused' : r.stage) : '–'" label="Rollout" :tone="r.stage === 'halted' ? 'bad' : r.paused ? 'warn' : r.stage === 'done' ? 'ok' : ''" />
      <Stat :value="fmt.n(onTarget)" :unit="`/ ${o.servers.length}`" label="Servers on it" />
      <Stat :value="fmt.n(upcoming)" label="Maintenance" :sub="upcoming ? 'booked, not ended' : 'nothing booked'" :tone="upcoming ? 'warn' : ''" />
    </template>
    <button v-if="u" type="button" @click="act('check')">Check GitHub now</button>
    <button class="primary" type="button" @click="toBooking">Book maintenance</button>
  </PageTop>
  <template v-if="u && o">
    <section class="panel">
      <header>
        <div><span class="label">Rollout</span><h2 class="big-title" style="margin-top: 2px">{{ r.target ? `Release ${r.target}` : 'Nothing rolled out yet' }}</h2></div>
        <div class="row">
          <span v-if="r.paused" class="pill warn">paused</span>
          <span v-if="r.pinned" class="pill">pinned</span>
          <span v-if="r.stage === 'halted'" class="pill bad">halted</span>
        </div>
      </header>
      <div v-if="r.target" class="stages">
        <div v-for="(st, i) in ORDER" :key="st" class="stage" :class="r.stage === 'halted' && i === Math.max(at, 0) ? 'halted' : i < at ? 'done' : i === at ? 'now' : ''">
          <b>{{ st[0].toUpperCase() + st.slice(1) }}</b>{{ i === at ? stageText[st] : '' }}
        </div>
      </div>
      <p v-if="r.note" class="callout" :class="{ bad: r.stage === 'halted' }">{{ r.note }}</p>
      <p v-if="r.target" class="small muted" style="margin-top: 8px">Since {{ fmt.when(r.stage_since) }} · previous release {{ r.previous || 'none recorded' }}</p>
      <div class="row" style="margin-top: 14px">
        <button v-if="r.paused" type="button" @click="act('resume')">Resume</button>
        <button v-else type="button" @click="act('pause')">Pause</button>
        <button v-if="r.pinned" type="button" @click="act('unpin')">Unpin (follow new releases)</button>
        <button v-else type="button" @click="act('pin')">Pin (hold this release)</button>
        <button v-if="['canary', 'verifying', 'halted'].includes(r.stage)" type="button" @click="act('promote', ['Update every server now?', 'Every server installs the release without waiting for the canary.', 'Skip the canary'])">Skip the canary</button>
        <button v-if="r.stage !== 'halted' && r.stage !== 'done' && r.target" type="button" @click="act('halt')">Halt</button>
        <button v-if="r.previous" class="danger" type="button" @click="act('rollback', [`Roll back to ${r.previous}?`, 'Every server goes back to the release before (each kept it), and the rollout is pinned there.', `Roll back to ${r.previous}`, true])">Roll back to {{ r.previous }}</button>
      </div>
    </section>
    <section class="panel">
      <header>
        <h2>Servers</h2>
        <span class="small muted">This machine's programs: <span class="pill" :class="verifyPill(u.verify)[0]" :title="verifyPill(u.verify)[2]">{{ verifyPill(u.verify)[1] }}</span></span>
      </header>
      <p class="small muted">Program: whether a server's programs were the signed release, byte for byte, when it last started (its updater checks, as root, before each start).</p>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Server</th><th>Runs</th><th>Program</th><th>Updates</th><th>Updater</th><th>Directory</th></tr></thead>
          <tbody>
            <tr v-for="sv in o.servers" :key="sv.id">
              <td>{{ serverName(sv) }}</td>
              <td><span class="pill" :class="updatePill(sv, r)[0]">{{ updatePill(sv, r)[1] }}</span></td>
              <td><span class="pill" :class="verifyPill(sv.update?.verify)[0]" :title="verifyPill(sv.update?.verify)[2]">{{ verifyPill(sv.update?.verify)[1] }}</span></td>
              <td><span class="pill" :class="sv.listing?.auto_update ? 'ok' : 'bad'">{{ sv.listing?.auto_update ? 'on' : 'off' }}</span></td>
              <td class="small muted">{{ updater(sv) }}</td>
              <td class="small"><span v-if="sv.delisted" class="pill warn" :title="sv.delisted">delisted</span><span v-else class="pill ok">listed</span></td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <section class="panel">
      <header><h2>Signed releases</h2></header>
      <div v-if="u.releases.length" class="table-wrap">
        <table>
          <thead><tr><th>Release</th><th>Published</th><th></th></tr></thead>
          <tbody>
            <tr v-for="rel in u.releases" :key="rel.version">
              <td><a :href="rel.page" target="_blank" rel="noopener noreferrer">{{ rel.version }}</a><span v-if="rel.version === r.target" class="pill ok" style="margin-left: 8px">target</span></td>
              <td class="muted">{{ rel.published_at ? new Date(rel.published_at).toLocaleString() : fmt.when(rel.seen_at) }}</td>
              <td class="r"><button v-if="rel.version !== r.target" class="small" type="button" @click="act('release', [`Roll out ${rel.version}?`, 'A canary first, then the rest. The rollout is pinned to it.', 'Roll this out'], { version: rel.version })">Roll this out</button></td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="empty">No signed release seen yet. Releases appear here once published and signed (scripts/sign-release.sh).</div>
    </section>
  </template>
  <div v-else class="empty">Loading…</div>
  <Maintenance :servers="o?.servers || []" :windows="maint?.windows || null" :error="maintError" @changed="reloadMaint" />
</template>

<script setup>
import { computed, onMounted, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import Stat from '../components/Stat.vue';
import Maintenance from '../components/Maintenance.vue';
import { api } from '../lib/api.js';
import { ensureOverview, useLoad } from '../lib/data.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt, serverName, updatePill, verifyPill } from '../lib/fmt.js';
import { live } from '../lib/live.js';
import { toast } from '../lib/ui.js';

const ORDER = ['canary', 'verifying', 'rolling', 'done'];
onMounted(() => ensureOverview().catch(() => {}));
const o = computed(() => live.overview);
const { data: u, reload } = useLoad(() => api('GET', '/updates'));
const r = computed(() => u.value.rollout);
const onTarget = computed(() => (o.value?.servers || []).filter(sv => r.value.target && sv.listing?.version === r.value.target).length);
// Maintenance windows: reloaded when any admin books or cancels one.
const { data: maint, error: maintError, reload: reloadMaint } = useLoad(() => api('GET', '/maintenance'), () => null, { refresh: false });
watch(() => live.maintenanceTick, reloadMaint);
const upcoming = computed(() => (maint.value?.windows || []).filter(w => !w.cancelled_at && w.end > Date.now() / 1000).length);
function toBooking() {
  const el = document.getElementById('maintenance');
  if (!el) return;
  el.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
  el.querySelector('input')?.focus({ preventScroll: true });
}
const at = computed(() => ORDER.indexOf(r.value.stage));
const name = id => serverName(o.value?.servers.find(sv => sv.id === id) || { id });
const stageText = computed(() => ({
  canary: r.value.canary ? `${name(r.value.canary)} installs it first` : 'choosing a server to try it on',
  verifying: `watching ${name(r.value.canary)} on the new release`,
  rolling: 'the rest update, each when it\'s quiet (or within 2 hours)',
  done: 'every server runs it',
}));
const updater = sv => {
  const up = sv.update?.updater || {};
  return up.state ? `${up.state}${up.version ? ' ' + up.version : ''}${up.at ? ' · ' + fmt.ago(up.at) : ''}${up.error ? ' · ' + up.error : ''}` : '–';
};
async function act(action, ask, body) {
  if (ask && !await confirmBox(ask[0], ask[1], ask[2], ask[3])) return;
  try { toast((await api('POST', `/updates/${action}`, body || {})).message); reload(); } catch (e) { toast(e.message, true); }
}
</script>
