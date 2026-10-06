// The live connection (GET /api/live, a WebSocket): the overview as it changes, a tick when
// servers report new numbers (charts fetch their latest points), and new audit entries.
// Reconnects on its own, backing off; pages read the reactive state below.
import { reactive } from 'vue';
import { session } from './session.js';

export const live = reactive({
  /** 'connecting', 'live', 'retrying', 'off' */
  status: 'off',
  overview: null,
  /** When the last overview arrived (ms). */
  at: 0,
  /** Goes up each time servers reported new numbers. */
  metricsTick: 0,
  /** Newest first, at most 50 since the page opened. */
  audit: [],
  /** Each server's live points of the last half hour: { server: [{ t, players, in_match, matches, lobbies, rx, tx, relayed }] }. */
  pulses: {},
  /** Recent events made from the pulses (counts only), newest first. */
  feed: [],
  /** Goes up when an alert is raised or resolved. */
  alertTick: 0,
  /** Goes up when a player's report comes, or an admin changes one. */
  reportTick: 0,
  /** Goes up when a maintenance window is booked or cancelled. */
  maintenanceTick: 0,
  /** Goes up when the roadmap or a suggestion changes. */
  roadmapTick: 0,
  /** Goes up when a support conversation changes (a player wrote, or an admin answered). */
  supportTick: 0,
});

const KEEP_POINTS = 180;

let socket = null;
let retry = 0;
let timer = null;
let wanted = false;

export function connectLive() {
  wanted = true;
  if (socket) return;
  live.status = retry ? 'retrying' : 'connecting';
  const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/live`;
  const ws = new WebSocket(url);
  socket = ws;
  ws.onopen = () => { retry = 0; live.status = 'live'; };
  ws.onmessage = ev => {
    let msg;
    try { msg = JSON.parse(ev.data); } catch { return; }
    if (msg.type === 'overview') {
      live.overview = msg.overview;
      live.at = Date.now();
      if (msg.metrics) live.metricsTick++;
    } else if (msg.type === 'audit') {
      live.audit = [msg.event, ...live.audit].slice(0, 50);
    } else if (msg.type === 'pulses') {
      live.pulses = msg.pulses || {};
      live.feed = msg.feed || [];
    } else if (msg.type === 'pulse') {
      const points = [...(live.pulses[msg.server] || []), msg.point].slice(-KEEP_POINTS);
      live.pulses = { ...live.pulses, [msg.server]: points };
    } else if (msg.type === 'event') {
      live.feed = [{ ...msg.event, fresh: true }, ...live.feed.map(e => ({ ...e, fresh: false }))].slice(0, 50);
    } else if (msg.type === 'alert') {
      // The overview that follows carries the open alerts; pages listening for alerts reload.
      live.alertTick++;
    } else if (msg.type === 'report') {
      // The overview that follows carries the open reports' count; the Reports page reloads.
      live.reportTick++;
    } else if (msg.type === 'maintenance') {
      live.maintenanceTick++;
    } else if (msg.type === 'roadmap') {
      live.roadmapTick++;
    } else if (msg.type === 'support') {
      live.supportTick++;
    } else if (msg.type === 'bye') {
      wanted = false;
      session.signedOut();
    }
  };
  ws.onclose = () => {
    socket = null;
    if (!wanted) { live.status = 'off'; return; }
    live.status = 'retrying';
    // 1, 2, 4 … 30 seconds; the session check on reconnect catches a sign-out.
    const wait = Math.min(30000, 1000 * 2 ** Math.min(retry++, 5));
    timer = setTimeout(connectLive, wait);
  };
}

export function disconnectLive() {
  wanted = false;
  clearTimeout(timer);
  retry = 0;
  if (socket) socket.close();
  socket = null;
  live.status = 'off';
  live.overview = null;
  live.audit = [];
  live.pulses = {};
  live.feed = [];
}
