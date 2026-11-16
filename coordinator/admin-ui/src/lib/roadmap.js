// The roadmap page's own pieces: the project's roadmap for this release, what bringing a
// network's up to it changes, and how statuses are coloured.

export const LANE_IDS = ['shipping', 'next', 'later', 'requested'];
export const PROMOTE_LANES = [['requested', 'Requested'], ['next', 'Next'], ['later', 'Later']];
export const AREAS = ['Launcher', 'Overlay', 'Server', 'Admin', 'Network'];
export const SUGGESTION_STATUSES = [['new', 'New'], ['planned', 'Planned'], ['done', 'Done'], ['declined', 'Declined']];
/** Statuses offered when editing an item (any other text up to 24 characters works too). */
export const STATUS_HINTS = ['In 0.5.0', 'In 0.5.1', 'In 0.5.2', 'In 0.5.3', 'In 0.5.4', 'In 0.5.5', 'Researching', 'Building', 'Mocked up', 'Planned', 'Later', 'On hold', 'When data says', 'Requested', 'Done'];
/** The lanes the project's roadmap fills; Requested is the players' and admins' own. */
export const PROJECT_LANES = ['shipping', 'next', 'later'];

/** Lane releases the project's roadmap sets. */
export const STARTING_RELEASES = { shipping: '0.5.4', next: '0.5.5' };

/** The project's roadmap as it stands for this release (STARTING_RELEASES.shipping): what an
 * empty roadmap starts from, and what "Update from the project's roadmap" brings one up to.
 * Requested items start hidden from players. */
export const STARTING_ITEMS = [
  ['shipping', 'Global matchmaking', 'One matchmaking server for everyone on the community network, so every search finds players from every region, and the game plays through the nearest of six relays: Falkenstein, Beauharnois, Silicon Valley, Santiago, Singapore and Sydney.', ['Server', 'Network'], 'In 0.5.0'],
  ['shipping', 'Fair play', 'Bans that hold across the network and for the PC, the game reporting a sped-up clock or changed code, other players\' games noticing impossible movement, Report a player from the overlay, and match recordings for the admins. Report only: people decide.', ['Overlay', 'Server'], 'In 0.5.0'],
  ['shipping', 'Crash and rubberbanding fixes', 'Six crashes in the game\'s own code patched, and the rubberbanding of long sessions in 0.4.6 and 0.4.7 found and fixed.', ['Overlay'], 'In 0.5.0'],
  ['shipping', 'A redesigned launcher', 'One native app for Windows, Linux and the Steam Deck, in a dark or a light look, with a Play button that says whether you\'re ready, and the overlay to match.', ['Launcher', 'Overlay'], 'In 0.5.0'],
  ['shipping', 'Matches, and lobbies that carry on', 'Every public match on a tab of its own, to join one, queue for a seat or start your own; the lobby carries on when its host leaves.', ['Launcher', 'Overlay'], 'In 0.5.0'],
  ['shipping', 'Fixes after the move to one network', 'The ping shown is your nearest relay\'s, in the launcher and the overlay; chat scrolls in the overlay; typing the community network\'s address works.', ['Launcher', 'Overlay'], 'In 0.5.1'],
  ['shipping', 'Joins that work', 'The server checks a forwarded port really lets others in before connecting them directly, and moves a player whose joins keep failing to the relay; two PCs behind one router both get through; the launcher learns which way works on each network you play on; the connection test names VPN adapters and lets the game through Windows Firewall.', ['Network', 'Launcher'], 'In 0.5.2'],
  ['shipping', 'Playing where UDP is blocked', 'The game goes through the server over HTTPS (port 443) on networks that block its own traffic, found by the launcher at each start.', ['Network', 'Launcher'], 'In 0.5.2'],
  ['shipping', 'Fair play, round two', 'The match\'s rules and every weapon\'s stats compared between the games in a match, so an edited one shows; cheat switches on a player\'s own character and live weapons; the checks report while a match runs. Report only.', ['Overlay', 'Server'], 'In 0.5.2'],
  ['shipping', 'Notices from the admins', 'A one-line notice above Play in every launcher within a minute (a known problem, a fix, news), and once in the overlay.', ['Launcher', 'Overlay'], 'In 0.5.2'],
  ['shipping', 'Start with Windows', 'The launcher can start when you sign in, minimised or in the tray, so friends and invitations reach you.', ['Launcher'], 'In 0.5.2'],
  ['shipping', 'Joins across relays, and crashes with Fusion Fix', 'Joining a host who plays directly on another relay works; a lobby nobody can get into on a quiet night goes through the relay after three players fail; the game no longer crashes at start with Fusion Fix installed; a co-op freeze after the weapons check is fixed.', ['Network', 'Overlay'], 'In 0.5.4'],
  ['shipping', 'Maintenance that says so', 'When the servers are closed for maintenance the launcher says until when, and the overlay warns before it starts.', ['Launcher', 'Server'], 'In 0.5.2'],
  ['next', 'Signing in through a nearby relay', 'For players whose connection to the matchmaking server in Canada loses its answers: the sign-in and the launcher\'s connection go through the relay near them instead.', ['Network', 'Launcher'], 'Planned'],
  ['next', 'Joining a match under way', 'Several players asked: whether a match can let players in after it starts, between rounds first. Being researched; a host\'s choice if it works.', ['Overlay', 'Server'], 'Researching'],
  ['next', 'Fair play: characters and names', 'Health and movement speed on a player\'s own character, names shown to others, rank and XP. Report only.', ['Overlay', 'Server'], 'Planned'],
  ['next', 'Matchmaking failover', 'Another server takes over matchmaking if the matchmaking server goes down.', ['Network'], 'Planned'],
  ['next', 'Screen shake and field of view', 'Options to turn the camera shake off and to widen the view, once their values are found in the game.', ['Launcher'], 'Planned'],
  ['next', 'Turning speed at high frame rates', 'The view turns slower with the mouse above 60 fps; a fix is being tested.', ['Launcher'], 'Building'],
  ['later', 'More relays where players are', 'Only once the Sessions page shows relayed players far from the six.', ['Network'], 'When data says'],
  ['later', 'Code signing for Windows', 'A signed launcher and DLL, so antivirus programs stop guessing.', ['Release'], 'Later'],
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
