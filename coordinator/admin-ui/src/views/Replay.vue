<template>
  <PageTop :title="title" :sub="sub">
    <template v-if="rec" #stats>
      <Stat :value="clock(rec.duration)" label="Recorded" :sub="rec.frames.length ? `${fmt.n(rec.frames.length)} samples` : 'no samples'" />
      <Stat :value="fmt.n(rec.pawns.size)" label="Characters" :sub="`${named} named`" />
      <Stat :value="fmt.n(rec.shots.length)" label="Shots" sub="from weapons' histories" />
      <Stat :value="fmt.n(kills.length)" label="Kills" sub="credited where seen" />
    </template>
    <RouterLink to="/sessions" class="small">← Sessions</RouterLink>
  </PageTop>

  <div v-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else-if="!rec" class="panel"><div class="empty">{{ waiting }}</div></div>
  <template v-else>
    <section class="panel">
      <header>
        <h2>{{ view === '3d' ? '3D' : 'Top down' }}</h2>
        <span class="small muted">{{ mapNote }}</span>
        <span class="spacer"></span>
        <div class="seg" role="group" aria-label="View">
          <button type="button" :aria-pressed="view === 'top'" @click="view = 'top'">Top down</button>
          <button type="button" :aria-pressed="view === '3d'" @click="view = '3d'">3D</button>
        </div>
      </header>
      <div class="viewer">
        <canvas v-show="view === 'top'" ref="canvas" class="map" :width="size.w" :height="size.h" role="img" :aria-label="`Characters at ${clock(t)}`"></canvas>
        <Replay3D v-if="view === '3d'" :rec="rec" :map="map" :t="t" :cut="cutOn ? cut : Infinity" :follow="follow" />
        <div v-if="view === '3d'" class="controls">
          <label class="small nowrap"><input v-model="cutOn" type="checkbox" /> Cut away above</label>
          <input v-model.number="cut" :disabled="!cutOn" type="range" :min="cutRange[0]" :max="cutRange[1]" step="10" aria-label="Height cut away above" />
          <span class="mono small nowrap">{{ Math.round(cut) }}</span>
          <label class="small">Follow
            <select v-model="follow">
              <option :value="null">nobody (drag to look around)</option>
              <option v-for="p in table" :key="p.id" :value="p.id">{{ p.name || `#${p.id}` }}</option>
            </select>
          </label>
          <label class="small file">{{ map ? 'Another map file…' : 'Load the map file…' }}
            <input type="file" accept=".femg" @change="loadMap" />
          </label>
        </div>
        <div class="controls">
          <button type="button" @click="toggle">{{ playing ? 'Pause' : 'Play' }}</button>
          <label class="small">Speed
            <select v-model.number="speed">
              <option v-for="s in [0.25, 0.5, 1, 2, 4, 8]" :key="s" :value="s">{{ s }}×</option>
            </select>
          </label>
          <input v-model.number="t" class="scrub" type="range" min="0" :max="rec.duration" step="100" aria-label="Time in the recording" />
          <span class="mono small nowrap">{{ clock(t) }} / {{ clock(rec.duration) }}</span>
        </div>
        <div v-if="view === 'top' && rec.box && rec.box.z1 - rec.box.z0 > 300" class="controls">
          <label class="small nowrap">Heights {{ Math.round(zFrom) }} to {{ Math.round(zTo) }}</label>
          <input v-model.number="zFrom" type="range" :min="rec.box.z0" :max="rec.box.z1" step="10" aria-label="Lowest height shown" />
          <input v-model.number="zTo" type="range" :min="rec.box.z0" :max="rec.box.z1" step="10" aria-label="Highest height shown" />
          <span class="small muted">others dimmed: one floor at a time</span>
        </div>
      </div>
    </section>

    <section class="panel">
      <header><h2>Characters</h2><span class="small muted">as {{ rec.header.recorder || 'the recording player' }}'s game saw them</span></header>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Who</th><th>Character</th><th>Team</th><th>Shots</th><th>Deaths</th><th>Kills</th><th>Fastest (units/s)</th></tr></thead>
          <tbody>
            <tr v-for="p in table" :key="p.id">
              <td><span class="swatch" :style="{ background: colour(p) }"></span> <b>{{ p.name || `#${p.id}` }}</b></td>
              <td class="small">{{ p.class }}</td>
              <td class="small">{{ p.team < 0 ? '?' : p.team }}</td>
              <td>{{ p.shots }}</td>
              <td>{{ p.deaths }}</td>
              <td>{{ p.kills }}</td>
              <td>{{ Math.round(p.fastest) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section class="panel">
      <header><h2>Look closer</h2><span class="small muted">each character's most, largest first: numbers to check, not verdicts</span></header>
      <div class="closer">
        <div v-for="[label, unit, list] in closer" :key="label">
          <h3 class="small">{{ label }}</h3>
          <div v-if="!list.length" class="small muted">none recorded</div>
          <ul v-else class="feed">
            <li v-for="x in list" :key="x.id"><button type="button" class="ghost small" @click="seek(x.t)"><span class="mono">{{ clock(x.t) }}</span> {{ nameOf(x.id) }}: {{ Math.round(x.value) }} {{ unit }}</button></li>
          </ul>
        </div>
      </div>
    </section>

    <section class="panel">
      <header><h2>What happened</h2><span class="small muted">click one to go there</span></header>
      <div v-if="!feed.length" class="empty">No rounds, deaths or kills recorded.</div>
      <ul v-else class="feed">
        <li v-for="(e, i) in feed" :key="i"><button type="button" class="ghost small" @click="seek(e.t)"><span class="mono">{{ clock(e.t) }}</span> {{ e.text }}</button></li>
      </ul>
    </section>
  </template>
</template>

<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import PageTop from '../components/PageTop.vue';
import Replay3D from '../components/Replay3D.vue';
import Stat from '../components/Stat.vue';
import { api, ApiError } from '../lib/api.js';
import { fmt } from '../lib/fmt.js';
import { keepMap, keptMap, readMap } from '../lib/mapgeo.js';
import { findings, frameAt, index, readRecording } from '../lib/replay.js';
import { session } from '../lib/session.js';

const props = defineProps({ server: { type: String, required: true }, player: { type: String, required: true }, id: { type: String, required: true } });

const rec = ref(null);
const error = ref('');
const waiting = ref('Asking the player\'s server for the recording…');
const canvas = ref(null);
const t = ref(0);
const playing = ref(false);
const speed = ref(1);
const zFrom = ref(0);
const zTo = ref(0);
const size = ref({ w: 900, h: 600 });

// The 3D view, and the map's geometry (map-geometry's file, from the admin's own game).
const view = ref('top');
const map = ref(null);
const cutOn = ref(true);
const cut = ref(0);
const follow = ref(null);
const cutRange = computed(() => (rec.value?.box ? [rec.value.box.z0 - 200, rec.value.box.z1 + 800] : [0, 1000]));
const mapNote = computed(() => {
  if (view.value === 'top') return 'positions only: the map is drawn in 3D';
  if (map.value) return `the map from your game (${map.value.name}): drag to turn, scroll to zoom, right-drag to move`;
  return `no map yet: make it with map-geometry from your game and load ${rec.value?.header.map || 'the map'}.femg`;
});
async function useMap(bytes, keep) {
  try {
    const m = await readMap(bytes);
    if (rec.value?.header.map && m.name.toLowerCase() !== rec.value.header.map.toLowerCase()) {
      error.value = `That's ${m.name}; this recording is on ${rec.value.header.map}.`;
      setTimeout(() => { error.value = ''; }, 4000);
      return;
    }
    map.value = Object.freeze(m);
    if (keep) await keepMap(m.name, bytes);
  } catch (e) {
    error.value = `Couldn't read the map file: ${e.message}`;
  }
}
async function loadMap(e) {
  const file = e.target.files?.[0];
  if (file) await useMap(new Uint8Array(await file.arrayBuffer()), true);
  e.target.value = '';
}
watch(view, async v => {
  if (v === '3d' && !map.value && rec.value?.header.map) {
    const kept = await keptMap(rec.value.header.map);
    if (kept) await useMap(kept, false);
  }
});

const title = computed(() => rec.value?.header.map ? `Replay: ${rec.value.header.map}` : 'Replay');
const sub = computed(() => {
  const h = rec.value?.header;
  if (!h) return `Recording ${props.id} on ${props.server}`;
  const when = h.started ? new Date(h.started * 1000).toLocaleString() : 'an unknown time';
  return `${(h.mode || 'unknown mode').replace(/^E|MatchManager$/g, '')} · recorded by ${h.recorder || '?'} at ${when} (their clock) · add-on ${h.build || '?'}`;
});
const named = computed(() => [...(rec.value?.pawns.values() || [])].filter(p => p.name).length);
const kills = computed(() => (rec.value?.events || []).filter(e => e.kind === 'kill'));
const nameOf = id => rec.value?.pawns.get(id)?.name || `#${id}`;
const table = computed(() => [...(rec.value?.pawns.values() || [])].map(p => ({ ...p, ...rec.value.tally.get(p.id) })).sort((a, b) => a.team - b.team || a.name.localeCompare(b.name)));
const feed = computed(() =>
  (rec.value?.events || [])
    .map(e => {
      if (e.kind === 'round') return { t: e.t, text: `Round: ${e.state.replace(/^RoundState_|^CoopMissionState_/, '') || 'none'}` };
      if (e.kind === 'death') return { t: e.t, text: `${nameOf(e.id)} died` };
      if (e.kind === 'kill') return { t: e.t, text: `${nameOf(e.killer)} killed ${nameOf(e.victim)}` };
      return null;
    })
    .filter(Boolean),
);

const closer = computed(() => {
  if (!rec.value) return [];
  const f = findings(rec.value);
  return [
    ['Sharpest turn just before a shot', '° in 100 ms', f.turns],
    ['Longest shot', 'm', f.shots],
    ['Fastest over a second', 'units/s', f.speeds],
  ];
});

const clock = ms => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

// Spies and mercs by team, others grey; the colours come from the page's own theme.
const css = name => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#888';
const colour = p => (p.team === 0 ? css('--info') : p.team === 1 ? css('--warn') : css('--muted'));

async function load() {
  // Asked for once (it says when it's here already), then fetched until the server sent it.
  try {
    const asked = await api('POST', `/recordings/${encodeURIComponent(props.server)}/${encodeURIComponent(props.player)}/${encodeURIComponent(props.id)}`);
    if (asked.status === 'pending') waiting.value = 'Asked the player\'s server for it: it comes with the server\'s next pulse or two…';
  } catch (e) {
    error.value = e.message;
    return;
  }
  for (let tries = 0; tries < 60 && !stopped; tries++) {
    const resp = await fetch(`/api/recordings/${encodeURIComponent(props.server)}/${encodeURIComponent(props.id)}`, { credentials: 'same-origin' });
    if (resp.status === 401) { session.signedOut(); return; }
    if (resp.ok && resp.status === 200) {
      try {
        const { header, records } = await readRecording(new Uint8Array(await resp.arrayBuffer()));
        rec.value = { header, ...index(records) };
        if (rec.value.box) { zFrom.value = rec.value.box.z0; zTo.value = rec.value.box.z1; cut.value = rec.value.box.z1 + 150; }
        await nextTick();
        fit();
        draw();
      } catch (e) {
        error.value = `Couldn't read the recording: ${e.message}`;
      }
      return;
    }
    const body = await resp.json().catch(() => ({}));
    if (resp.status === 202 && (body.status === 'failed' || body.status === 'expired')) {
      error.value = body.message || 'The server couldn\'t send it.';
      return;
    }
    if (resp.status !== 202) {
      error.value = new ApiError(resp.status, body).message;
      return;
    }
    await new Promise(r => setTimeout(r, 3000));
  }
  if (!rec.value && !error.value && !stopped) error.value = 'The server hasn\'t sent it in three minutes: it may be offline. Try again later.';
}

// Playback: the clock moves with the frames the browser draws.
let raf = 0;
let last = 0;
let stopped = false;
function frame(now) {
  if (!playing.value) return;
  if (last) t.value = Math.min(rec.value.duration, t.value + (now - last) * speed.value);
  last = now;
  if (t.value >= rec.value.duration) playing.value = false;
  else raf = requestAnimationFrame(frame);
}
function toggle() {
  if (!rec.value) return;
  if (!playing.value && t.value >= rec.value.duration) t.value = 0;
  playing.value = !playing.value;
  last = 0;
  if (playing.value) raf = requestAnimationFrame(frame);
}
function seek(at) {
  t.value = Math.max(0, at - 2000);
  window.scrollTo({ top: 0, behavior: 'smooth' });
}
const keys = e => {
  if (e.code === 'Space' && !['INPUT', 'SELECT', 'BUTTON', 'TEXTAREA'].includes(document.activeElement?.tagName)) {
    e.preventDefault();
    toggle();
  }
};

function fit() {
  const el = canvas.value;
  if (!el) return;
  const w = Math.max(320, Math.min(el.parentElement.clientWidth, 1400));
  size.value = { w, h: Math.round(w * 0.62) };
}

// World (x, y; z up) to canvas, keeping its shape, with a margin.
function projector() {
  const b = rec.value.box;
  const { w, h } = size.value;
  const pad = 30;
  const sx = (w - pad * 2) / Math.max(1, b.x1 - b.x0);
  const sy = (h - pad * 2) / Math.max(1, b.y1 - b.y0);
  const s = Math.min(sx, sy);
  const ox = (w - (b.x1 - b.x0) * s) / 2;
  const oy = (h - (b.y1 - b.y0) * s) / 2;
  return { s, at: ([x, y]) => [ox + (x - b.x0) * s, oy + (y - b.y0) * s] };
}

function draw() {
  const el = canvas.value;
  const r = rec.value;
  if (!el || !r || !r.box) return;
  const g = el.getContext('2d');
  const { w, h } = size.value;
  g.clearRect(0, 0, w, h);
  g.fillStyle = css('--panel-2');
  g.fillRect(0, 0, w, h);
  const p = projector();
  const inFloor = z => z >= zFrom.value - 1 && z <= zTo.value + 1;
  const i = frameAt(r.frames, t.value);
  // Scale: a 10 m bar (1000 units).
  g.strokeStyle = css('--muted');
  g.fillStyle = css('--muted');
  g.lineWidth = 2;
  g.beginPath(); g.moveTo(16, h - 16); g.lineTo(16 + 1000 * p.s, h - 16); g.stroke();
  g.font = `12px ${css('--sans') || 'sans-serif'}`;
  g.fillText('10 m', 16, h - 22);
  if (i == null) return;
  // Trails: the last 3 seconds.
  const from = frameAt(r.frames, t.value - 3000) ?? 0;
  const trails = new Map();
  for (let k = from; k <= i; k++) for (const c of r.frames[k].pawns) {
    if (!trails.has(c.id)) trails.set(c.id, []);
    trails.get(c.id).push(c.at);
  }
  for (const [id, pts] of trails) {
    const who = r.pawns.get(id) || { team: -1 };
    g.strokeStyle = colour(who);
    g.globalAlpha = 0.35;
    g.lineWidth = 2;
    g.beginPath();
    pts.forEach((pt, k) => {
      const [x, y] = p.at(pt);
      // A jump (a respawn) breaks the line.
      if (k && Math.hypot(pt[0] - pts[k - 1][0], pt[1] - pts[k - 1][1]) > 1500) g.moveTo(x, y);
      else if (k) g.lineTo(x, y);
      else g.moveTo(x, y);
    });
    g.stroke();
  }
  g.globalAlpha = 1;
  // Shots in the last second, fading.
  for (const s of r.shots) {
    if (s.t > t.value || s.t < t.value - 1000) continue;
    const who = r.pawns.get(s.id) || { team: -1 };
    g.globalAlpha = (inFloor(s.from[2]) ? 0.9 : 0.25) * (1 - (t.value - s.t) / 1000);
    g.strokeStyle = colour(who);
    g.lineWidth = 1.5;
    const [ax, ay] = p.at(s.from);
    const [bx, by] = p.at(s.to);
    g.beginPath(); g.moveTo(ax, ay); g.lineTo(bx, by); g.stroke();
    g.beginPath(); g.arc(bx, by, 3, 0, Math.PI * 2); g.stroke();
  }
  g.globalAlpha = 1;
  // The characters: where, facing, alive or not, firing.
  for (const c of r.frames[i].pawns) {
    const who = r.pawns.get(c.id) || { team: -1, name: '' };
    const [x, y] = p.at(c.at);
    const yaw = (c.yaw / 65536) * Math.PI * 2;
    g.globalAlpha = inFloor(c.at[2]) ? 1 : 0.2;
    g.strokeStyle = colour(who);
    g.fillStyle = colour(who);
    g.lineWidth = 2;
    g.beginPath(); g.moveTo(x, y); g.lineTo(x + Math.cos(yaw) * 16, y + Math.sin(yaw) * 16); g.stroke();
    g.beginPath(); g.arc(x, y, 6, 0, Math.PI * 2);
    if (c.health > 0) g.fill(); else g.stroke();
    if (c.firing) { g.beginPath(); g.arc(x, y, 10, 0, Math.PI * 2); g.stroke(); }
    g.fillStyle = css('--text');
    g.fillText(who.name || `#${c.id}`, x + 9, y - 8);
  }
  g.globalAlpha = 1;
}

watch([t, zFrom, zTo, size], () => draw());
const onResize = () => { fit(); draw(); };
onMounted(() => {
  load();
  window.addEventListener('keydown', keys);
  window.addEventListener('resize', onResize);
});
onBeforeUnmount(() => {
  stopped = true;
  cancelAnimationFrame(raf);
  window.removeEventListener('keydown', keys);
  window.removeEventListener('resize', onResize);
});
</script>

<style scoped>
.viewer { display: grid; gap: 10px; }
.map { width: 100%; height: auto; border-radius: var(--radius); border: 1px solid var(--line); }
.spacer { flex: 1 1 auto; }
.seg { display: inline-flex; gap: 4px; }
.seg button[aria-pressed='true'] { background: var(--accent-soft); color: var(--text); }
.file { position: relative; cursor: pointer; text-decoration: underline; }
.file input { position: absolute; inset: 0; opacity: 0; cursor: pointer; }
.controls { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
.scrub { flex: 1 1 240px; min-width: 0; }
.swatch { display: inline-block; width: 10px; height: 10px; border-radius: 50%; vertical-align: middle; }
.closer { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 16px; }
.closer h3 { margin: 0 0 6px; }
.feed { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; max-height: 360px; overflow: auto; }
.feed button { width: 100%; text-align: left; color: var(--text); }
</style>
