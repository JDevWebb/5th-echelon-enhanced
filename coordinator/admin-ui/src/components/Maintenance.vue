<template>
  <section id="maintenance" class="panel">
    <header>
      <h2 class="big-title">Book maintenance</h2>
      <span class="small muted">Times are in your time zone: <span class="mono">{{ zone }}</span></span>
    </header>
    <p class="small muted lead-in">A notice only: nothing is stopped. On the day, launchers on the server warn their players in their own time, and rollouts don't update a server during its window.</p>
    <form class="book" @submit.prevent="book">
      <span id="mt-servers" class="label">Servers</span>
      <div class="row" role="group" aria-labelledby="mt-servers">
        <label v-for="sv in servers" :key="sv.id" class="check">
          <input v-model="picked" type="checkbox" :value="sv.id"> {{ serverName(sv) }} <small>{{ short(sv.id) }}</small>
        </label>
        <label class="check">
          <input v-model="network" type="checkbox"> Whole network <small>the coordinator</small>
        </label>
      </div>

      <span id="mt-start" class="label">Starts</span>
      <div class="row" role="group" aria-labelledby="mt-start">
        <input v-model="date" type="date" aria-label="Start date" required class="short">
        <input v-model="time" type="time" aria-label="Start time" required class="short">
        <span class="small muted mono">{{ start ? `= ${utcText(start)}` : '' }}</span>
      </div>

      <span id="mt-length" class="label">Length</span>
      <div class="row" role="group" aria-labelledby="mt-length">
        <button v-for="[m, text] in LENGTHS" :key="m" class="chip" type="button" :aria-pressed="String(!other && minutes === m)" @click="other = false; minutes = m">{{ text }}</button>
        <button class="chip" type="button" :aria-pressed="String(other)" @click="other = true">Other</button>
        <label v-if="other" class="row other">
          <input v-model.number="otherMinutes" type="number" min="1" max="1440" step="1" aria-label="Length in minutes" class="short">
          <span class="small muted">minutes (up to 24 hours)</span>
        </label>
      </div>

      <label class="label" for="mt-note">For players</label>
      <div class="stack" style="gap: 6px">
        <input id="mt-note" v-model="note" type="text" maxlength="100" placeholder="Moving to a faster machine" class="note">
        <span class="small muted">Optional. Shown after the time in the launcher. {{ note.length }}/100</span>
      </div>

      <span class="label">Players see</span>
      <div class="preview">
        <template v-if="preview.length">
          On the day, in their own time:
          <em v-for="(line, i) in preview" :key="i" class="line">{{ line }}</em>
        </template>
        <span v-else class="faint">Pick a server, or the whole network, to see what players are told.</span>
      </div>

      <div class="row end actions">
        <span v-if="problem" class="small warn-text" role="status">{{ problem }}</span>
        <button class="primary" type="submit" :disabled="busy || !!problem">Book maintenance</button>
      </div>
    </form>
  </section>

  <section class="panel">
    <header>
      <h2>Booked</h2>
      <span class="small muted">Cancelling takes the warning off launchers within a minute.</span>
    </header>
    <div v-if="error" class="empty">
      <p>Bookings aren't available here yet.</p>
      <p class="small err">{{ error }}</p>
    </div>
    <div v-else-if="!windows" class="empty">Loading…</div>
    <div v-else-if="!windows.length" class="empty">Nothing booked. Book a window above and it shows here.</div>
    <div v-else class="table-wrap">
      <table>
        <thead><tr><th>Server</th><th>When (yours)</th><th>Length</th><th>Shown to players</th><th>Status</th><th></th></tr></thead>
        <tbody>
          <tr v-for="w in windows" :key="w.id">
            <td>{{ w.server_name }}<span class="sub">{{ w.server ? short(w.server) : 'coordinator' }}</span></td>
            <td>{{ localText(w.start, w.end) }}<span class="sub">{{ utcRange(w.start, w.end) }}</span></td>
            <td class="mono">{{ lengthShort(w.end - w.start) }}</td>
            <td><span v-if="w.note">{{ w.note }}</span><span v-else class="faint">nothing</span></td>
            <td><span class="pill" :class="status(w)[0]" :title="status(w)[2]">{{ status(w)[1] }}</span></td>
            <td class="r">
              <span v-if="confirming === w.id" class="row end confirm" role="group" :aria-label="`Cancel ${w.server_name}'s maintenance?`">
                <span class="small">Cancel it?</span>
                <button class="danger small" type="button" :disabled="busy" @click="cancel(w)">Yes, cancel</button>
                <button class="ghost small" type="button" @click="confirming = null">Keep</button>
              </span>
              <button v-else-if="!w.cancelled_at && w.end > now" class="ghost small" type="button" @click="confirming = w.id">Cancel</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<script setup>
// Maintenance windows (Updates › Maintenance): book one for servers or the whole network, in
// the admin's own time zone, and see or cancel what's booked.
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { api } from '../lib/api.js';
import { serverName } from '../lib/fmt.js';
import { toast } from '../lib/ui.js';

const props = defineProps({ servers: { type: Array, default: () => [] }, windows: Array, error: String });
const emit = defineEmits(['changed']);

const LENGTHS = [[15, '15 min'], [30, '30 min'], [60, '1 hour'], [120, '2 hours']];
const pad = n => String(n).padStart(2, '0');
const localDate = d => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
// The next whole hour, by default.
const next = new Date(Date.now() + 3600e3);
next.setMinutes(0, 0, 0);

const picked = ref([]);
const network = ref(false);
const date = ref(localDate(next));
const time = ref(`${pad(next.getHours())}:00`);
const minutes = ref(60);
const other = ref(false);
const otherMinutes = ref(90);
const note = ref('');
const busy = ref(false);
const confirming = ref(null);

const now = ref(Date.now() / 1000);
let timer;
onMounted(() => { timer = setInterval(() => { now.value = Date.now() / 1000; }, 30000); });
onUnmounted(() => clearInterval(timer));

// The zone's short name and offset, e.g. "NZDT (UTC+13)".
const zone = (() => {
  const d = new Date();
  const name = new Intl.DateTimeFormat(undefined, { timeZoneName: 'short' }).formatToParts(d).find(p => p.type === 'timeZoneName')?.value || '';
  const off = -d.getTimezoneOffset();
  const utc = `UTC${off < 0 ? '−' : '+'}${Math.floor(Math.abs(off) / 60)}${Math.abs(off) % 60 ? ':' + pad(Math.abs(off) % 60) : ''}`;
  return name && !/^(GMT|UTC)[+−-]/.test(name) ? `${name} (${utc})` : utc;
})();

const start = computed(() => {
  if (!date.value || !time.value) return null;
  const t = new Date(`${date.value}T${time.value}`).getTime();
  return Number.isFinite(t) ? Math.floor(t / 1000) : null;
});
const length = computed(() => (other.value ? Math.round(Number(otherMinutes.value) || 0) : minutes.value));

// What stops it being booked, in words; empty when it can be.
const problem = computed(() => {
  if (!picked.value.length && !network.value) return 'Pick at least one server, or the whole network.';
  if (start.value == null) return 'Pick a date and a time.';
  if (start.value < now.value - 300) return 'That time has passed.';
  if (start.value > now.value + 90 * 86400) return 'Book up to 90 days ahead.';
  if (!(length.value >= 1 && length.value <= 1440)) return 'A window lasts from 1 minute to 24 hours.';
  return '';
});

// As players' launchers word it.
const clock = t => new Date(t * 1000).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
function lengthWords(m) {
  if (m < 60) return `about ${m} minute${m === 1 ? '' : 's'}`;
  const halves = Math.round(m / 30);
  const whole = Math.floor(halves / 2);
  if (halves % 2) return `about ${whole}½ hours`;
  return `about ${whole} hour${whole === 1 ? '' : 's'}`;
}
// A server's short name: its host's first label (oceania, na1), else its id.
function short(id) {
  const host = props.servers.find(sv => sv.id === id)?.listing?.host || '';
  const first = host.split(':')[0].split('.')[0];
  return first && /[a-z]/i.test(first) && host.includes('.') ? first : id;
}
const city = sv => serverName(sv).split(',')[0];
const preview = computed(() => {
  if (start.value == null) return [];
  const at = clock(start.value);
  const forHow = lengthWords(length.value);
  const why = note.value.trim() ? `: ${note.value.trim()}` : '';
  const lines = [];
  const first = props.servers.find(sv => picked.value.includes(sv.id));
  if (first) lines.push(`${city(first)} goes down at ${at} for ${forHow}${why}.`);
  if (network.value) lines.push(`Friends on other servers, the server list and new names pause from ${at} for ${forHow}. Your games carry on.`);
  return lines;
});

async function book() {
  if (problem.value || busy.value) return;
  busy.value = true;
  try {
    const r = await api('POST', '/maintenance', {
      servers: picked.value, network: network.value, start: start.value, end: start.value + length.value * 60, note: note.value.trim(),
    });
    const n = r.windows?.length || 0;
    toast(n === 1 ? 'Maintenance booked.' : `Maintenance booked: ${n} windows.`);
    picked.value = [];
    network.value = false;
    note.value = '';
    emit('changed');
  } catch (e) {
    toast(e.status === 404 ? 'This coordinator doesn\'t take bookings yet.' : e.message, true);
  } finally {
    busy.value = false;
  }
}

async function cancel(w) {
  busy.value = true;
  try {
    await api('DELETE', `/maintenance/${encodeURIComponent(w.id)}`);
    toast(`${w.server_name}'s maintenance is cancelled.`);
    confirming.value = null;
    emit('changed');
  } catch (e) {
    toast(e.message, true);
  } finally {
    busy.value = false;
  }
}

// The booked table.
const sameDay = (a, b, utc) => {
  const o = utc ? { timeZone: 'UTC' } : {};
  const f = t => new Date(t * 1000).toLocaleDateString('en-CA', o);
  return f(a) === f(b);
};
const day = (t, utc) => new Date(t * 1000).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', ...(utc ? { timeZone: 'UTC' } : {}) });
const utcClock = t => new Date(t * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone: 'UTC' });
function localText(a, b) {
  return sameDay(a, b) ? `${day(a)}, ${clock(a)}–${clock(b)}` : `${day(a)}, ${clock(a)} – ${day(b)}, ${clock(b)}`;
}
function utcRange(a, b) {
  const local = day(a) !== day(a, true) ? `${day(a, true)}, ` : '';
  return sameDay(a, b, true) ? `${local}${utcClock(a)}–${utcClock(b)} UTC` : `${day(a, true)}, ${utcClock(a)} – ${day(b, true)}, ${utcClock(b)} UTC`;
}
function utcText(t) {
  return `${utcClock(t)} UTC, ${day(t, true)}`;
}
function lengthShort(secs) {
  const m = Math.round(secs / 60);
  if (m < 60) return `${m} min`;
  return m % 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m / 60} h`;
}
/** [pill class, label, what it means] */
function status(w) {
  if (w.cancelled_at) return ['', 'Cancelled', `Cancelled ${new Date(w.cancelled_at * 1000).toLocaleString()}`];
  if (now.value >= w.end) return ['', 'Done', 'It has ended.'];
  if (now.value >= w.start) return ['bad', 'Under way', 'Launchers on it say it\'s down for maintenance.'];
  if (w.start - now.value <= 12 * 3600 || sameDay(w.start, now.value)) return ['warn', 'Warning', 'Launchers are warning their players now.'];
  return ['info', 'Booked', 'Launchers warn players from midnight on the day, or 12 hours before.'];
}
</script>

<style scoped>
.lead-in { margin: -4px 0 16px; max-width: 80ch; }
.book { display: grid; grid-template-columns: 150px minmax(0, 1fr); gap: 14px 18px; align-items: start; }
.book > .label { padding-top: 9px; }
.check { display: inline-flex; gap: 8px; align-items: center; background: var(--panel-2); border: 1px solid var(--line); border-radius: 8px; padding: 6px 12px; color: var(--soft); cursor: pointer; }
.check small { color: var(--faint); font-family: var(--mono); }
.short { width: auto; }
.other { gap: 8px; }
.other input { width: 100px; }
.note { max-width: 420px; }
.preview { background: var(--panel-2); border: 1px dashed var(--line); border-radius: 8px; padding: 10px 12px; color: var(--soft); }
.preview .line { display: block; color: var(--text); }
.actions { grid-column: 1 / -1; }
.warn-text { color: var(--warn); }
td .sub { display: block; color: var(--faint); font: 12px var(--mono); }
.confirm { gap: 6px; flex-wrap: nowrap; }
@media (max-width: 760px) {
  .book { grid-template-columns: minmax(0, 1fr); }
  .book > .label { padding-top: 0; }
}
</style>
