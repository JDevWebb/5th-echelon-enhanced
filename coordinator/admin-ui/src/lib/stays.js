// A player's stays in rooms (the Sessions report's `rooms`), in words: shared by Sessions,
// Live and a match's page.
import { fmt } from './fmt.js';

export const MODES = { coop: 'co-op', svm: 'Spies vs Mercs' };

/** Seconds under a minute: a stay of 20 s isn't "0m". */
export const dur = secs => (secs > 0 && secs < 60 ? `${secs} s` : fmt.dur(secs));

/** The name of a map or game mode (`labels` from the API: the game's, or the admins'). */
export const labelOf = (labels, kind, id) => (id == null ? null : (labels || []).find(l => l.kind === kind && l.id === id)?.name);

/** "Pakistani Embassy (Charlie (Extraction))": a room's map and game mode, named where known. */
export function where(labels, r) {
  const map = labelOf(labels, 'map', r.map) || (r.map != null ? `map #${r.map}` : '');
  const mode = labelOf(labels, 'game_mode', r.game_mode);
  return map ? `${map}${mode ? ` (${mode})` : ''}` : '';
}

/** "public co-op match", "private Spies vs Mercs match" or "party". */
export function roomKind(r) {
  const kind = r.kind ?? r.room_kind;
  return kind === 'match' ? `${r.private ? 'private' : 'public'} ${MODES[r.mode] || ''} match`.replace(/ +/g, ' ') : 'party';
}

/** Who else was there, and for how long when only part of the stay. */
export function withText(r) {
  if (!r.with.length) return 'alone';
  return 'with ' + r.with.map(n => (r.together?.[n] != null ? `${n} (for ${dur(r.together[n])})` : n)).join(', ');
}

/** "Joined Oni's public co-op match on Pakistani Embassy (Charlie (Extraction)), with …"
 * (without the map where the page already names it: `{ map: false }`). */
export function stayTitle(labels, r, { map = true } = {}) {
  const kind = roomKind(r);
  const whose = r.host ? `Opened a ${kind}` : `Joined ${r.host_name ? r.host_name + "'s" : 'a'} ${kind}`;
  const on = map && r.kind === 'match' && where(labels, r) ? ` on ${where(labels, r)}` : '';
  return `${whose}${on}, ${withText(r)}`;
}

/** How a stay ended, in words. */
export function endText(r) {
  const e = r.end || {};
  const what = r.kind === 'match' ? 'match' : 'party';
  const after = dur(r.to - r.from) || '<1 s';
  switch (e.how) {
    case 'still': return `Still in the ${what} (${after} so far)`;
    case 'removed': return `Removed from the ${what} by ${e.by_name || 'the host'} after ${after}`;
    case 'dropped': return `Lost: the game's connection dropped, after ${after} in the ${what}`;
    case 'restarted': return `Lost: the game restarted or crashed, after ${after} in the ${what}`;
    case 'signed_out': return `${e.signout === 'timed_out' ? 'Timed out' : 'Signed out'} after ${after} in the ${what}`;
    case 'server_restart': return `The server restarted, after ${after} in the ${what}`;
    case 'abandoned': return r.host ? `Closed the ${what} after ${after}` : `Left the ${what} after ${after}`;
    default: return `Left the ${what} after ${after}${e.ended ? '; it ended' : ''}`;
  }
}

export function endClass(r) {
  const how = r.end?.how;
  return how === 'restarted' || how === 'dropped' ? 'bad' : how === 'removed' || how === 'server_restart' ? 'warn' : '';
}

/** How a guest's game reached the host's, as the server saw it when they joined. */
export function netText(n) {
  if (!n || !('relayed' in n)) return '';
  const ms = v => (v != null ? `${v} ms` : '?');
  const pings = `ping ${ms(n.ping_ms)}, host ${ms(n.host_ping_ms)}`;
  if (!n.relayed) return `Direct · ${pings}`;
  const best = n.direct_ms != null ? ` (direct at best ${n.direct_ms} ms)` : '';
  return `Relayed: ${ms(n.relay_ms)} round trip${best} · ${pings}`;
}

const PROBLEM_KINDS = new Set(['signin_refused', 'join_failed', 'relay_drop', 'request_error', 'nat_missing', 'nat_lost', 'report_refused', 'restart']);

/** A timeline mark's colour. */
export function markClass(m) {
  if (m.kind === 'client_log') return /^Game error/.test(m.text) ? 'bad' : /^Game warn/.test(m.text) ? 'warn' : 'muted';
  if (m.kind === 'restart') return 'warn';
  if (PROBLEM_KINDS.has(m.kind)) return m.kind === 'request_error' || m.kind === 'report_refused' ? 'warn' : 'bad';
  if (m.kind === 'stats' || m.kind === 'log_sent') return 'ok';
  if (m.kind === 'search') return 'info';
  return 'muted';
}

/** A match's page: `#/match/<server>/<room>/<since>` (the room's id comes again after the
 * server restarts; `since` tells them apart). */
export const matchLink = (server, room, since) => `/match/${encodeURIComponent(server)}/${room}/${since || 0}`;
