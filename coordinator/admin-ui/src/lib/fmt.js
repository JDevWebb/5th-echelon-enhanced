// Formatting for the dashboards.

export const fmt = {
  n: (v, d = 0) => v == null || Number.isNaN(v) ? '–' : Number(v).toLocaleString(undefined, { maximumFractionDigits: d, minimumFractionDigits: d }),
  pct: v => v == null ? '–' : `${Math.round(v)}%`,
  bytes(v) {
    if (v == null) return '–';
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (Math.abs(v) >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return `${v.toFixed(v < 10 && i ? 1 : 0)} ${u[i]}`;
  },
  /** Data sizes the way hosts bill them: 1 GB = 1000 MB. */
  size(v) {
    if (v == null) return '–';
    const u = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
    let i = 0;
    while (Math.abs(v) >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return `${v.toFixed(v < 10 && i ? 2 : v < 100 && i ? 1 : 0)} ${u[i]}`;
  },
  rate: v => v == null ? '–' : fmt.bits(v * 8),
  bits(v) {
    const u = ['bps', 'kbps', 'Mbps', 'Gbps'];
    let i = 0;
    while (Math.abs(v) >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return `${v.toFixed(v < 10 && i ? 1 : 0)} ${u[i]}`;
  },
  ms: v => v == null ? '–' : `${Math.round(v)} ms`,
  ago(t) {
    if (!t) return 'never';
    const d = Math.max(0, Date.now() / 1000 - t);
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)} min ago`;
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
    return `${Math.floor(d / 86400)} d ago`;
  },
  dur(secs) {
    if (!secs) return '–';
    const d = Math.floor(secs / 86400), hr = Math.floor(secs % 86400 / 3600), m = Math.floor(secs % 3600 / 60);
    return d ? `${d}d ${hr}h` : hr ? `${hr}h ${m}m` : `${m}m`;
  },
  when: t => t ? new Date(t * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '–',
};

export function flag(cc) {
  return /^[A-Z]{2}$/.test(cc || '') ? String.fromCodePoint(...[...cc].map(c => 0x1f1a5 + c.charCodeAt(0))) : '';
}

export function browserName(ua) {
  const b = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  const os = /iPhone|iPad/.test(ua) ? 'iOS' : /Android/.test(ua) ? 'Android' : /Mac OS/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  return `${b}${os ? ' on ' + os : ''}`;
}

export const serverName = sv => sv?.listing?.name || sv?.id || '';

/** [css class, label] for a server's state. */
export function serverStatus(sv) {
  if (!sv.online) return ['bad', 'offline'];
  if (sv.delisted) return ['warn', 'delisted'];
  return ['ok', 'online'];
}

/** The pill for a server's release: [css class, text]. */
export function updatePill(sv, rollout) {
  const v = sv.listing?.version || '?';
  const u = sv.update?.updater || {};
  if (!sv.listing?.auto_update) return ['warn', `${v} · updates off`];
  if (u.state === 'updating') return ['', `${v} → ${u.version}`];
  if (u.state === 'failed' || u.state === 'rolled-back') return ['bad', `${v} · ${u.state} ${u.version || ''}`];
  if (rollout?.target && v !== rollout.target) return ['warn', `${v} · behind`];
  return ['ok', v];
}

export const PALETTE = ['#8fd14f', '#5aa9e6', '#f0b44c', '#c79bff', '#ff8f70', '#5fd7d0', '#e6e36a', '#f07cc4'];
const known = new Map();
export function colorFor(id) {
  if (!known.has(id)) known.set(id, PALETTE[known.size % PALETTE.length]);
  return known.get(id);
}

/** Lines per server for `field` from a /series answer. */
export function seriesFor(data, field, servers = [], transform = v => v) {
  const names = new Map(servers.map(sv => [sv.id, serverName(sv)]));
  return Object.keys(data.points || {}).sort().map(id => ({
    name: names.get(id) || id,
    color: colorFor(id),
    points: data.points[id].map(p => ({ t: p.t, v: transform(p[field], p) })),
  }));
}

/** One line adding up every server's `field`. */
export function totalSeries(data, field, name = 'All servers') {
  const sums = new Map();
  for (const pts of Object.values(data.points || {})) for (const p of pts) sums.set(p.t, (sums.get(p.t) || 0) + (p[field] || 0));
  return { name, color: 'var(--text)', points: [...sums.entries()].sort((a, b) => a[0] - b[0]).map(([t, v]) => ({ t, v })) };
}

export const RANGES = [[3600, '1 h'], [21600, '6 h'], [86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '1 y']];
export const PLACE_RANGES = [[0, 'Now'], [86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '1 y']];

/** The network's live numbers now, and over the last half hour, from the servers' pulses. */
export function liveTotals(pulses) {
  const now = { players: 0, in_match: 0, matches: 0, bps: 0, relayed: 0, servers: 0 };
  const series = new Map();
  for (const points of Object.values(pulses || {})) {
    const last = points.at(-1);
    if (!last || Date.now() / 1000 - last.t > 45) continue;
    now.servers++;
    now.players += last.players;
    now.in_match += last.in_match;
    now.matches += last.matches;
    now.bps += (last.rx + last.tx) * 8;
    now.relayed += last.relayed * 8;
    // Points land at different seconds on each server: 10-second slots.
    for (const p of points) {
      const k = Math.floor(p.t / 10) * 10;
      const e = series.get(k) || { t: k, players: 0, bps: 0 };
      e.players += p.players;
      e.bps += (p.rx + p.tx) * 8;
      series.set(k, e);
    }
  }
  return { now, series: [...series.values()].sort((a, b) => a.t - b.t) };
}

/** What players can tick in a report. */
export const PROBLEMS = {
  join: 'Couldn\'t join',
  lag: 'Lag',
  crash: 'Crash',
  connection: 'Connection',
  version: 'Game version',
  signin: 'Sign-in',
  other: 'Other',
};
