// The roadmap page's own pieces: the project's roadmap for this release, what bringing a
// network's up to it changes, and how statuses are coloured.

export const LANE_IDS = ['shipping', 'next', 'later', 'requested'];
export const PROMOTE_LANES = [['requested', 'Requested'], ['next', 'Next'], ['later', 'Later']];
export const AREAS = ['Launcher', 'Overlay', 'Server', 'Admin', 'Network'];
export const SUGGESTION_STATUSES = [['new', 'New'], ['planned', 'Planned'], ['done', 'Done'], ['declined', 'Declined']];
/** Statuses offered when editing an item (any other text up to 24 characters works too). */
export const STATUS_HINTS = ['In 0.4.5', 'In 0.4.6', 'Building', 'Mocked up', 'Planned', 'Later', 'On hold', 'When data says', 'Requested', 'Done'];
/** The lanes the project's roadmap fills; Requested is the players' and admins' own. */
export const PROJECT_LANES = ['shipping', 'next', 'later'];

/** Lane releases the project's roadmap sets. */
export const STARTING_RELEASES = { shipping: '0.4.5', next: '0.4.6' };

/** The project's roadmap as it stands for this release (STARTING_RELEASES.shipping): what an
 * empty roadmap starts from, and what "Update from the project's roadmap" brings one up to.
 * Requested items start hidden from players. */
export const STARTING_ITEMS = [
  ['shipping', 'Fixes from players\' reports', 'No more duplicate matches in the lists after a host leaves, and game logs that keep what went wrong, so hangs and crashes can be traced.', ['Server', 'Launcher', 'Overlay'], 'In 0.4.5'],
  ['shipping', 'Steam Input for controllers', 'A Steam copy on Windows can be started through Steam, so Steam\'s controller settings apply.', ['Launcher'], 'In 0.4.5'],
  ['next', 'Chat and who\'s online', 'Find people to play with from the launcher, with popups in the game. Community network only.', ['Launcher', 'Overlay'], 'In 0.4.6'],
  ['next', 'Coordinator failover', 'Move the coordinator to a one-level name so Cloudflare can proxy it, then turn failover on.', ['Ops'], 'In 0.4.6'],
  ['next', 'Fewer relayed matches', 'Relayed players connect straight to hosts that have a port open.', ['Network'], 'Planned'],
  ['later', 'Global matchmaking', 'One matchmaking server for everyone, relays kept near players, hosts ranked by ping.', ['Server', 'Network'], 'Later'],
  ['later', 'Lobby browser', 'A list of every open public lobby (mode, map, players and seats), alongside global matchmaking.', ['Launcher', 'Overlay'], 'Later'],
  ['later', 'Relays in US West and Brazil', 'Only once the Sessions page shows relayed players there.', ['Network'], 'When data says'],
  ['later', 'Code signing for Windows', 'A signed launcher and DLL, so antivirus programs stop guessing.', ['Release'], 'Later'],
  ['later', 'Settings of our own', 'Stop the two launchers resetting each other\'s settings.', ['Launcher'], 'On hold'],
  // Requests quote players: admins only until someone chooses to show them. (0.4.2's two are
  // answered: disconnects by Sessions' endings, chat under Next.)
].map(([lane, title, body, tags, status, source = '']) => ({ lane, title, body, tags, status, public: lane !== 'requested', source }));

/** What "Update from the project's roadmap" sends: the project's items for Shipping, Next and
 * Later, in order, and the lanes' releases. */
export function projectRoadmap(starting = STARTING_ITEMS, releases = STARTING_RELEASES) {
  return { items: starting.filter(i => PROJECT_LANES.includes(i.lane)), releases };
}

const titleKey = t => (t || '').trim().toLowerCase();
const byOrder = (a, b) => ((a.position || 0) - (b.position || 0)) || (a.id - b.id);
/** An item's fields that differ from the project's, by the names the preview shows. */
function differing(old, item) {
  const fields = [];
  const tags = t => (t || []).map(s => s.trim()).join('\n');
  if ((old.title || '') !== item.title.trim()) fields.push('title');
  if ((old.body || '') !== (item.body || '').trim()) fields.push('text');
  if (tags(old.tags) !== tags(item.tags)) fields.push('tags');
  if ((old.status || '') !== (item.status || '').trim()) fields.push('status');
  if (Boolean(old.public) !== Boolean(item.public)) fields.push(item.public ? 'now public' : 'now hidden');
  if ((old.source || '') !== (item.source || '').trim()) fields.push('who asked');
  return fields;
}

/** What bringing the roadmap's items up to the project's would change, matched as the
 * coordinator matches them (roadmap.rs `update_from_project`): an item in Shipping, Next or Later
 * with the project item's title (whatever its case) becomes that item, the project's others are
 * added, and the rest go. Requested, and items made of players' suggestions, stay.
 * Answers { added, moved: [{item, from, fields}], changed: [{item, fields}], removed, kept, same }. */
export function projectChanges(current, starting = STARTING_ITEMS) {
  const ours = (current || []).filter(i => PROJECT_LANES.includes(i.lane));
  const open = ours.filter(i => !i.suggestion).sort(byOrder);
  const kept = ours.filter(i => i.suggestion);
  const out = { added: [], moved: [], changed: [], removed: [], kept, same: [] };
  for (const item of starting.filter(i => PROJECT_LANES.includes(i.lane))) {
    const at = open.findIndex(i => titleKey(i.title) === titleKey(item.title));
    if (at < 0) { out.added.push(item); continue; }
    const [old] = open.splice(at, 1);
    const fields = differing(old, item);
    if (old.lane !== item.lane) out.moved.push({ item, from: old.lane, fields });
    else if (fields.length) out.changed.push({ item, fields });
    else out.same.push(item);
  }
  out.removed = open;
  return out;
}

/** The pill class for an item's status. */
export function statusClass(status) {
  const s = (status || '').toLowerCase();
  if (/^in \d|^shipped|^done|^released/.test(s)) return 'ok';
  if (/build|mocked|progress|testing/.test(s)) return 'warn';
  if (/request/.test(s)) return 'info';
  return '';
}
