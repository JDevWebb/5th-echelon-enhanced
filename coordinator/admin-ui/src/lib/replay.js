// Match recordings (the replay crate's format, read here as the Rust reader does): gzip
// around "FERP", a version (u16), the header, then records, each a tag byte, its time (u32
// ms) and fields, little endian; a string is a u8 length and UTF-8. A file cut short (the
// game ended mid-write) reads up to the cut.

const VERSION = 1;

/** The bytes inside a gzip stream: what came before a cut, if it's cut. */
async function gunzip(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
  const reader = stream.getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      size += value.length;
    }
  } catch (e) {
    if (!size) throw e;
  }
  const out = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) { out.set(c, at); at += c.length; }
  return out;
}

class Cursor {
  constructor(bytes) { this.b = bytes; this.v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength); this.at = 0; this.text = new TextDecoder(); }
  has(n) { return this.at + n <= this.b.length; }
  u8() { const x = this.v.getUint8(this.at); this.at += 1; return x; }
  u16() { const x = this.v.getUint16(this.at, true); this.at += 2; return x; }
  i16() { const x = this.v.getInt16(this.at, true); this.at += 2; return x; }
  u32() { const x = this.v.getUint32(this.at, true); this.at += 4; return x; }
  i32() { const x = this.v.getInt32(this.at, true); this.at += 4; return x; }
  u64() { const lo = this.u32(); const hi = this.u32(); return hi * 2 ** 32 + lo; }
  f32() { const x = this.v.getFloat32(this.at, true); this.at += 4; return x; }
  f3() { return [this.f32(), this.f32(), this.f32()]; }
  str() { const n = this.u8(); if (!this.has(n)) throw new RangeError(); const s = this.text.decode(this.b.subarray(this.at, this.at + n)); this.at += n; return s; }

  record() {
    const tag = this.u8();
    const t = this.u32();
    switch (tag) {
      case 1: return { kind: 'pawn', t, id: this.u16(), class: this.str(), name: this.str(), team: this.i32() };
      case 2: {
        const n = this.u8();
        const pawns = [];
        for (let i = 0; i < n; i++) pawns.push({ id: this.u16(), at: this.f3(), yaw: this.u16(), pitch: this.u16(), health: this.i16(), firing: this.u8() !== 0 });
        return { kind: 'frame', t, pawns };
      }
      case 3: return { kind: 'shot', t, id: this.u16(), from: this.f3(), to: this.f3(), game_time: this.f32() };
      case 4: return { kind: 'death', t, id: this.u16() };
      case 5: return { kind: 'kill', t, killer: this.u16(), victim: this.u16() };
      case 6: return { kind: 'round', t, state: this.str() };
      case 7: return { kind: 'gone', t, id: this.u16() };
      default: return null;
    }
  }
}

/** A recording's `{ header, records }` from its file's bytes. */
export async function readRecording(bytes) {
  const c = new Cursor(await gunzip(bytes));
  if (!c.has(6) || String.fromCharCode(c.u8(), c.u8(), c.u8(), c.u8()) !== 'FERP') throw new Error('not a match recording');
  const version = c.u16();
  if (version !== VERSION) throw new Error(`a recording of version ${version}; this page reads ${VERSION}`);
  const header = { started: c.u64(), recorder: c.str(), map: c.str(), mode: c.str(), build: c.str() };
  const records = [];
  for (;;) {
    try {
      const r = c.record();
      if (!r) break;
      records.push(r);
    } catch {
      // A cut in the middle of a record: what came before stands.
      break;
    }
  }
  return { header, records };
}

/** What the viewer draws from: the characters, frames in time order, and the events. */
export function index(records) {
  const pawns = new Map();
  const frames = [];
  const shots = [];
  const events = [];
  for (const r of records) {
    if (r.kind === 'pawn') pawns.set(r.id, { ...pawns.get(r.id), id: r.id, class: r.class, name: r.name || pawns.get(r.id)?.name || '', team: r.team });
    else if (r.kind === 'frame') frames.push(r);
    else if (r.kind === 'shot') shots.push(r);
    else events.push(r);
  }
  const duration = Math.max(0, ...records.map(r => r.t));
  // Each character's numbers, for the table beside the map.
  const tally = new Map([...pawns.keys()].map(id => [id, { shots: 0, deaths: 0, kills: 0, fastest: 0 }]));
  const of = id => { if (!tally.has(id)) tally.set(id, { shots: 0, deaths: 0, kills: 0, fastest: 0 }); return tally.get(id); };
  for (const s of shots) of(s.id).shots++;
  for (const e of events) {
    if (e.kind === 'death') of(e.id).deaths++;
    if (e.kind === 'kill') of(e.killer).kills++;
  }
  // The fastest each went across the ground over a second; a jump past 1500 units is a respawn.
  const trail = new Map();
  for (const f of frames) {
    for (const p of f.pawns) {
      const across = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1]);
      let q = trail.get(p.id);
      if (!q) trail.set(p.id, (q = []));
      if (q.length && across(q[q.length - 1][1], p.at) > 1500) q.length = 0;
      q.push([f.t, p.at]);
      while (q.length && f.t - q[0][0] > 1000) q.shift();
      const span = (f.t - q[0][0]) / 1000;
      if (span >= 0.9) of(p.id).fastest = Math.max(of(p.id).fastest, across(p.at, q[0][1]) / span);
    }
  }
  // The area played in (x and y), and the heights seen.
  let box = null;
  for (const f of frames) {
    for (const p of f.pawns) {
      const [x, y, z] = p.at;
      if (!box) box = { x0: x, x1: x, y0: y, y1: y, z0: z, z1: z };
      box.x0 = Math.min(box.x0, x); box.x1 = Math.max(box.x1, x);
      box.y0 = Math.min(box.y0, y); box.y1 = Math.max(box.y1, y);
      box.z0 = Math.min(box.z0, z); box.z1 = Math.max(box.z1, z);
    }
  }
  return { pawns, frames, shots, events, duration, tally, box };
}

/** The last frame at or before `t` (binary search). */
export function frameAt(frames, t) {
  let lo = 0;
  let hi = frames.length - 1;
  if (hi < 0 || frames[0].t > t) return null;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (frames[mid].t <= t) lo = mid; else hi = mid - 1;
  }
  return lo;
}

/** What's worth a closer look, largest first (no verdicts: the numbers a real match makes
 * aren't known yet): the sharpest turn just before each shot (degrees in the 200 ms before
 * it, an aimbot's snap), the longest shots (metres), and the fastest a character moved over
 * a second (as witness reports measure it). Each with when, to jump there. */
export function findings(rec, most = 8) {
  const turns = [];
  const yawAt = (id, t) => {
    const i = frameAt(rec.frames, t);
    return i == null ? null : rec.frames[i].pawns.find(p => p.id === id)?.yaw ?? null;
  };
  for (const s of rec.shots) {
    // The turn over each 100 ms step of the 200 ms before the shot; the largest step counts.
    let sharpest = 0;
    for (let dt = 200; dt > 0; dt -= 100) {
      const a = yawAt(s.id, s.t - dt);
      const b = yawAt(s.id, s.t - dt + 100);
      if (a == null || b == null) continue;
      const d = Math.abs(((b - a + 98304) % 65536) - 32768);
      sharpest = Math.max(sharpest, (d / 65536) * 360);
    }
    turns.push({ t: s.t, id: s.id, value: sharpest });
  }
  const shots = rec.shots.map(s => ({ t: s.t, id: s.id, value: Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1], s.to[2] - s.from[2]) / 100 }));
  // Speeds: over each second, across the ground; a jump past 1500 units is a respawn.
  const speeds = [];
  const trail = new Map();
  for (const f of rec.frames) {
    for (const p of f.pawns) {
      let q = trail.get(p.id);
      if (!q) trail.set(p.id, (q = []));
      if (q.length && Math.hypot(q[q.length - 1][1][0] - p.at[0], q[q.length - 1][1][1] - p.at[1]) > 1500) q.length = 0;
      q.push([f.t, p.at]);
      while (q.length && f.t - q[0][0] > 1000) q.shift();
      const span = (f.t - q[0][0]) / 1000;
      if (span >= 0.9) speeds.push({ t: f.t, id: p.id, value: Math.hypot(p.at[0] - q[0][1][0], p.at[1] - q[0][1][1]) / span });
    }
  }
  // Each character's worst only, then the worst of those.
  const top = list => {
    const best = new Map();
    for (const x of list) if (!best.has(x.id) || x.value > best.get(x.id).value) best.set(x.id, x);
    return [...best.values()].sort((a, b) => b.value - a.value).slice(0, most);
  };
  return { turns: top(turns), shots: top(shots), speeds: top(speeds) };
}

/** The most two recordings of one match may be apart, and the longest a merged match runs. */
const MOST_APART = 10 * 60 * 1000;
const LONGEST = 3 * 3600 * 1000;

/** Several players' recordings of one match as one: `{ header, records, sources, left }` for
 * `index`. `owners[i]` is the name of the account that sent recording i (from the server,
 * not the recording's own word). Each player's character comes from their own recording
 * (their game moves it) when exactly one recording is that account's; a character nobody
 * recorded, or two recordings claim, comes from the recording that saw it in the most
 * samples. Recordings are lined up by when the round started (every game sees it within
 * moments), else by their start times; one more than [`MOST_APART`] from the first is left
 * out (`left`). Shots come from the recording the shooter's frames come from; deaths and
 * kills once each. `sources` says whose recording each character came from, and for a
 * player's own track how far the others' games saw them from it (`apart`, game units: a
 * doctored track, or rubberbanding, shows as distance). */
export function merge(recordings, owners = []) {
  const lower = s => (s || '').toLowerCase();
  const started = r => r.records.find(x => x.kind === 'round' && /Started$/.test(x.state))?.t;
  const anchor = recordings.find(r => started(r) != null);
  const rawOffsets = recordings.map(r => {
    if (anchor && started(r) != null) return started(anchor) - started(r);
    return (r.header.started - recordings[0].header.started) * 1000;
  });
  const left = [];
  const keep = recordings.map((r, i) => {
    const ok = Number.isFinite(rawOffsets[i]) && Math.abs(rawOffsets[i]) <= MOST_APART;
    if (!ok) left.push(owners[i] || r.header.recorder || `recording ${i + 1}`);
    return ok;
  });
  const offsets = rawOffsets.map((o, i) => (keep[i] ? o : 0));
  // Characters by name across recordings; an unnamed one stays its recording's own.
  const names = new Map();
  const keyOf = (i, p) => (p.name ? `name:${lower(p.name)}` : `rec${i}:${p.id}`);
  const perRec = recordings.map((r, i) => {
    const pawns = new Map();
    const samples = new Map();
    if (!keep[i]) return { pawns, samples };
    for (const x of r.records) {
      if (x.kind === 'pawn') pawns.set(x.id, { ...pawns.get(x.id), ...x, name: x.name || pawns.get(x.id)?.name || '' });
      if (x.kind === 'frame') for (const p of x.pawns) samples.set(p.id, (samples.get(p.id) || 0) + 1);
    }
    for (const p of pawns.values()) if (!names.has(keyOf(i, p))) names.set(keyOf(i, p), { name: p.name, class: p.class, team: p.team });
    return { pawns, samples };
  });
  // Who each character's frames come from: its own player's recording (exactly one), else the most samples.
  const source = new Map();
  for (const key of names.keys()) {
    const seen = [];
    recordings.forEach((r, i) => {
      for (const p of perRec[i].pawns.values()) {
        if (keyOf(i, p) !== key) continue;
        seen.push({ rec: i, id: p.id, samples: perRec[i].samples.get(p.id) || 0, own: !!(p.name && owners[i] && lower(p.name) === lower(owners[i])) });
      }
    });
    const own = seen.filter(s => s.own);
    const conflict = own.length > 1;
    const pick = own.length === 1 ? own[0] : seen.sort((a, b) => b.samples - a.samples)[0];
    if (pick) source.set(key, { ...pick, own: own.length === 1, conflict, seenBy: seen.filter(s => s.rec !== pick.rec) });
  }
  const ids = new Map([...names.keys()].map((k, n) => [k, n + 1]));
  const out = [];
  for (const [key, info] of names) out.push({ kind: 'pawn', t: 0, id: ids.get(key), class: info.class, name: info.name, team: info.team });
  const trackOf = (rec, id) => {
    const list = [];
    for (const x of recordings[rec].records) {
      if (x.kind !== 'frame') continue;
      const p = x.pawns.find(q => q.id === id);
      if (p) list.push({ t: x.t + offsets[rec], p });
    }
    return list;
  };
  // Frames on a common 100 ms clock: each character's sample nearest to each tick.
  const tracks = new Map();
  for (const [key, s] of source) tracks.set(key, trackOf(s.rec, s.id));
  const from = Math.max(0, Math.min(0, ...[...tracks.values()].map(l => l[0]?.t ?? 0)));
  const to = Math.min(from + LONGEST, Math.max(0, ...[...tracks.values()].map(l => l[l.length - 1]?.t ?? 0)));
  const at = new Map([...tracks.keys()].map(k => [k, 0]));
  for (let t = from; t <= to; t += 100) {
    const pawns = [];
    for (const [key, list] of tracks) {
      let i = at.get(key);
      while (i + 1 < list.length && list[i + 1].t <= t) i++;
      at.set(key, i);
      const s = list[i];
      // Only while it was there (within half a second of a sample).
      if (s && Math.abs(s.t - t) <= 500) pawns.push({ ...s.p, id: ids.get(key) });
    }
    out.push({ kind: 'frame', t, pawns });
  }
  // How far the others saw each own-recorded character from its own track: the median, over
  // the seconds both have it, of the distance across the ground.
  const nearest = (list, t) => {
    let lo = 0;
    let hi = list.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (list[mid].t <= t) lo = mid; else hi = mid - 1;
    }
    return list[lo] && Math.abs(list[lo].t - t) <= 300 ? list[lo].p : null;
  };
  const apart = new Map();
  for (const [key, s] of source) {
    if (!s.own || !s.seenBy.length) continue;
    const mine = tracks.get(key);
    const theirs = s.seenBy.map(o => trackOf(o.rec, o.id));
    const d = [];
    for (let t = from; t <= to; t += 1000) {
      const a = nearest(mine, t);
      if (!a) continue;
      for (const list of theirs) {
        const b = list.length ? nearest(list, t) : null;
        if (b) d.push(Math.hypot(a.at[0] - b.at[0], a.at[1] - b.at[1]));
      }
    }
    if (d.length) apart.set(key, d.sort((x, y) => x - y)[d.length >> 1]);
  }
  // Shots from the recording each character's frames come from; deaths and kills once each.
  const seenEvents = new Set();
  const anchorAt = recordings.indexOf(anchor ?? recordings[0]);
  recordings.forEach((r, i) => {
    if (!keep[i]) return;
    const local = id => {
      const p = perRec[i].pawns.get(id);
      return p ? keyOf(i, p) : null;
    };
    for (const x of r.records) {
      const t = x.t + offsets[i];
      if (t < from || t > to) continue;
      if (x.kind === 'shot') {
        const key = local(x.id);
        const s = source.get(key);
        // From the recording its frames come from: one shot, once.
        if (s && s.rec === i) out.push({ ...x, t, id: ids.get(key) });
      } else if (x.kind === 'death' || x.kind === 'kill') {
        const a = local(x.kind === 'death' ? x.id : x.killer);
        const b = x.kind === 'kill' ? local(x.victim) : '';
        const tag = `${x.kind}:${a}:${b}:${Math.round(t / 2000)}`;
        if (!a || seenEvents.has(tag)) continue;
        seenEvents.add(tag);
        out.push(x.kind === 'death' ? { kind: 'death', t, id: ids.get(a) } : { kind: 'kill', t, killer: ids.get(a), victim: ids.get(b) });
      } else if (x.kind === 'round' && i === anchorAt) {
        out.push({ ...x, t });
      }
    }
  });
  out.sort((a, b) => a.t - b.t || (a.kind === 'pawn' ? -1 : 0));
  const header = { ...recordings[0].header, recorder: owners.filter(Boolean).join(', ') || recordings.map(r => r.header.recorder).filter(Boolean).join(', ') };
  const sources = new Map(
    [...source].map(([key, s]) => [ids.get(key), { recorder: owners[s.rec] || recordings[s.rec].header.recorder, own: s.own, conflict: s.conflict, apart: apart.get(key) ?? null }]),
  );
  return { header, records: out, sources, left };
}
