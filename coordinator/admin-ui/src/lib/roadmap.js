// The roadmap page's own pieces: the project's starting roadmap, the prompt for Claude Code
// each item and suggestion hands out, and how statuses are coloured.

export const LANE_IDS = ['shipping', 'next', 'later', 'requested'];
export const PROMOTE_LANES = [['requested', 'Requested'], ['next', 'Next'], ['later', 'Later']];
export const AREAS = ['Launcher', 'Overlay', 'Server', 'Admin', 'Network'];
export const SUGGESTION_STATUSES = [['new', 'New'], ['planned', 'Planned'], ['done', 'Done'], ['declined', 'Declined']];
/** Statuses offered when editing an item (any other text up to 24 characters works too). */
export const STATUS_HINTS = ['In 0.4.2', 'Building', 'Mocked up', 'Planned', 'Later', 'On hold', 'When data says', 'Requested', 'Done'];

/** Lane releases the starting roadmap sets. */
export const STARTING_RELEASES = { shipping: '0.4.2', next: '0.4.3' };

/** The project's roadmap as it stood for 0.4.2, for an empty roadmap to start from. Requested
 * items start hidden from players. */
export const STARTING_ITEMS = [
  ['shipping', 'Game diagnostics', 'Warnings and errors from players\' games, so failed joins can be traced on their side.', ['Launcher', 'Server'], 'In 0.4.2'],
  ['shipping', 'Server menu on Play', 'Community servers with your ping, one click to switch, with your name and friends.', ['Launcher'], 'In 0.4.2'],
  ['shipping', '"Other players can\'t reach you"', 'The overlay says when the NAT helper loses the game, and what to do.', ['Overlay'], 'In 0.4.2'],
  ['shipping', 'Maintenance windows', 'Book downtime for a server or the whole network; from 12 hours ahead, launchers warn its players in their own time.', ['Admin', 'Launcher'], 'In 0.4.2'],
  ['shipping', 'Maintenance warnings in the game', 'An overlay toast 10 minutes before the server or the network goes down, for players mid-match.', ['Overlay'], 'In 0.4.2'],
  ['shipping', 'Roadmap and suggestions', 'This roadmap, kept by admins and shown in the launcher, with players\' suggestions sent from the launcher.', ['Admin', 'Launcher'], 'In 0.4.2'],
  ['shipping', 'Admin UI redesign', 'The admin UI in the launcher\'s frame: an icon rail, a hero and a band with each page\'s key numbers.', ['Admin'], 'In 0.4.2'],
  ['next', 'Global matchmaking', 'One matchmaking server for everyone, relays kept near players, hosts ranked by ping.', ['Server', 'Network'], 'Planned'],
  ['next', 'Coordinator failover', 'Move the coordinator to a one-level name so Cloudflare can proxy it, then turn failover on.', ['Ops'], 'Planned'],
  ['next', 'Fewer relayed matches', 'Relayed players connect straight to hosts that have a port open.', ['Network'], 'Planned'],
  ['later', 'Chat and who\'s online', 'Find people to play with from the launcher, with popups in the game. Community network only.', ['Launcher', 'Overlay'], '0.4.4'],
  ['later', 'Relays in US West and Brazil', 'Only once the Sessions page shows relayed players there.', ['Network'], 'When data says'],
  ['later', 'Code signing for Windows', 'A signed launcher and DLL, so antivirus programs stop guessing.', ['Release'], 'Later'],
  ['later', 'Settings of our own', 'Stop the two launchers resetting each other\'s settings.', ['Launcher'], 'On hold'],
  ['requested', 'Chat to find players', '"A chat in the launcher to find people to play with."', ['Launcher'], 'Requested', 'Player report · 6 Oct · now under Later'],
  ['requested', 'Reports explain disconnects', '"Got disconnected around every 30 to 40 mins."', ['Server'], 'Requested', 'Discord · na1 · 5 Oct'],
  // Requests quote players: admins only until someone chooses to show them.
].map(([lane, title, body, tags, status, source = '']) => ({ lane, title, body, tags, status, public: lane !== 'requested', source }));

/** The pill class for an item's status. */
export function statusClass(status) {
  const s = (status || '').toLowerCase();
  if (/^in \d|^shipped|^done|^released/.test(s)) return 'ok';
  if (/build|mocked|progress|testing/.test(s)) return 'warn';
  if (/request/.test(s)) return 'info';
  return '';
}

const STEPS = `Read docs/roadmap.md and the code it touches first. Then:
1. Say what players and admins would see, and what changes where.
2. Mock it up before building anything.
3. List the decisions you need from me, with a recommendation for each.
Don't push or deploy anything.`;

const REPO = 'In ~/Development/Projects/5th-echelon-standalone (5th Echelon Enhanced)';

/** A ready-to-paste prompt for Claude Code to start on a roadmap item. */
export function itemPrompt(item) {
  const what = item.lane === 'requested' ? 'requested feature' : 'roadmap item';
  return `${REPO}, look into this ${what}:

${item.title}
${item.body || ''}
${item.source ? `Asked for: ${item.source}\n` : ''}Area: ${(item.tags || []).join(', ') || 'not set'}
Status: ${item.status || 'not set'}

${STEPS}`;
}

/** The same for a player's suggestion. */
export function suggestionPrompt(s) {
  const when = s.created_at ? new Date(s.created_at * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' }) : '';
  const from = [`Suggested by ${s.name || 'a player'}`, s.server ? `on ${s.server}` : '', s.launcher ? `(launcher ${s.launcher})` : '', when ? `· ${when}` : ''].filter(Boolean).join(' ');
  return `${REPO}, look into this suggestion from a player:

${s.title}
${s.text}
${from}
Area: ${s.area || 'Other'}

${STEPS}`;
}
