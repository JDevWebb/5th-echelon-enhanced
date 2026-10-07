// The roadmap page's own pieces: the project's starting roadmap, and how statuses are
// coloured.

export const LANE_IDS = ['shipping', 'next', 'later', 'requested'];
export const PROMOTE_LANES = [['requested', 'Requested'], ['next', 'Next'], ['later', 'Later']];
export const AREAS = ['Launcher', 'Overlay', 'Server', 'Admin', 'Network'];
export const SUGGESTION_STATUSES = [['new', 'New'], ['planned', 'Planned'], ['done', 'Done'], ['declined', 'Declined']];
/** Statuses offered when editing an item (any other text up to 24 characters works too). */
export const STATUS_HINTS = ['In 0.4.3', 'In 0.4.4', 'Building', 'Mocked up', 'Planned', 'Later', 'On hold', 'When data says', 'Requested', 'Done'];

/** Lane releases the starting roadmap sets. */
export const STARTING_RELEASES = { shipping: '0.4.3', next: '0.4.4' };

/** The project's roadmap as it stood for 0.4.3, for an empty roadmap to start from. Requested
 * items start hidden from players. */
export const STARTING_ITEMS = [
  ['shipping', 'Support', 'Write to the admins from the launcher, with your logs; their answer shows there and in the game.', ['Launcher'], 'In 0.4.3'],
  ['shipping', 'Replies to reports', 'The admins answer a report; you read it under Settings › Feedback › Your reports.', ['Launcher', 'Admin'], 'In 0.4.3'],
  ['shipping', 'Choose the overlay\'s key', 'F5 does nothing on most laptops: pick another key. It opens only with the game in front.', ['Overlay'], 'In 0.4.3'],
  ['shipping', 'Security review', 'The whole network attacked and fixed: no taking a server down, locking players out, or reaching you through the relay.', ['Server', 'Network'], 'In 0.4.3'],
  ['shipping', 'Live, and how each stay ended', 'Every game open now, and why each player\'s session ended, so drops can be traced.', ['Admin'], 'In 0.4.3'],
  ['shipping', 'The log before a problem', 'When a join fails or a connection drops, the game\'s last 15 minutes of log, private details hidden.', ['Launcher', 'Admin'], 'In 0.4.3'],
  ['next', 'Chat and who\'s online', 'Find people to play with from the launcher, with popups in the game. Community network only.', ['Launcher', 'Overlay'], '0.4.4'],
  ['next', 'Public matches outlive their host', 'When the host leaves, the match carries on for everyone else.', ['Server'], 'Planned'],
  ['next', 'Roadmap from the project', 'One click brings a network\'s roadmap up to the project\'s for each release, keeping players\' requests.', ['Admin'], '0.4.4'],
  ['next', 'Global matchmaking', 'One matchmaking server for everyone, relays kept near players, hosts ranked by ping.', ['Server', 'Network'], 'Planned'],
  ['next', 'Coordinator failover', 'Move the coordinator to a one-level name so Cloudflare can proxy it, then turn failover on.', ['Ops'], 'Planned'],
  ['next', 'Fewer relayed matches', 'Relayed players connect straight to hosts that have a port open.', ['Network'], 'Planned'],
  ['later', 'Relays in US West and Brazil', 'Only once the Sessions page shows relayed players there.', ['Network'], 'When data says'],
  ['later', 'Code signing for Windows', 'A signed launcher and DLL, so antivirus programs stop guessing.', ['Release'], 'Later'],
  ['later', 'Settings of our own', 'Stop the two launchers resetting each other\'s settings.', ['Launcher'], 'On hold'],
  // Requests quote players: admins only until someone chooses to show them. (0.4.2's two are
  // answered: disconnects by Sessions' endings, chat under Next.)
].map(([lane, title, body, tags, status, source = '']) => ({ lane, title, body, tags, status, public: lane !== 'requested', source }));

/** The pill class for an item's status. */
export function statusClass(status) {
  const s = (status || '').toLowerCase();
  if (/^in \d|^shipped|^done|^released/.test(s)) return 'ok';
  if (/build|mocked|progress|testing/.test(s)) return 'warn';
  if (/request/.test(s)) return 'info';
  return '';
}
