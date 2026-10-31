// The way to the Rust side (src-tauri/src/commands.rs) and to the window. In a plain browser
// (`npm run dev`, or a preview), made-up answers instead, so the screens can be looked at alone.
const tauri = window.__TAURI__;
export const inTauri = Boolean(tauri);

const sample = {
  info: {
    product: '5th Echelon Enhanced', release: '0.4.8-dev', game_dir: 'C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM',
    server: 'eu1.scbl.jdevwebb.net', account: 'Kiwi', community: true, play_mode: 'play', managed: false, running: false,
    connecting: false, max_name: 24, identity: 'DNIC-JULS', beta: location.search.includes('beta') ? 1 : 0,
    name_not_allowed: location.search.includes('banners') ? 'Kiwi' : null, emulator_notice: location.search.includes('banners'),
  },
  questions: {
    diagnostics: {
      title: 'Help us fix connection problems',
      what: '5th Echelon now sends the server you play on a short diagnostic log while you play: the game’s warnings, errors and network messages.',
      private: 'Before anything leaves your PC, your PC’s name, your Windows account name, your user folder and your internet address are removed.',
      later: 'You can change this any time in Settings › Feedback.',
    },
    options: [
      { id: 'mouse-accel', title: 'Mouse moves 1:1', text: 'Fast flicks turn as far as slow ones.', recommended: true, on: true, blocked: null },
      { id: 'autoclimb', title: 'No automatic climbing', text: 'Sam stops climbing by himself when you sprint into edges.', recommended: false, on: false, blocked: null },
    ],
    options_intro: 'This update adds options that change how the game plays. Each is set as we recommend; tick or untick any you like.',
    options_later: 'Change these any time in Settings › Game.',
  },
  diagnostics_example: ['[12:01:04] WARN net: join timed out after 20 s', '[12:01:05] INFO lobby: back to the menu'],
  activity: null,
  find_games: ['C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM'],
  servers: {
    network: 'https://scbl.jdevwebb.net',
    header: 'Community servers · scbl.jdevwebb.net',
    community: true,
    card: { caption: 'Server', caption_why: null, place: 'Falkenstein, Germany', ping: 38, players: 24 },
    note: null,
    rows: [
      { host: 'eu1.scbl.jdevwebb.net', place: 'Falkenstein, Germany', name: 'EU', title: '5th Echelon Community EU (Falkenstein, Germany)', ping: 38, players: 24, playing: 'SvM Classic 6/8 · 3 in lobbies', current: true, best: true, friends_only: false, maintenance: null },
      { host: 'na1.scbl.jdevwebb.net', place: 'Beauharnois, Canada', name: 'North America', title: '5th Echelon Community North America (Beauharnois, Canada)', ping: 112, players: 9, playing: '', current: false, best: false, friends_only: true, maintenance: null },
      { host: 'oceania.scbl.jdevwebb.net', place: 'Sydney, Australia', name: 'Oceania', title: '5th Echelon Community Oceania (Sydney, Australia)', ping: 290, players: 4, playing: '', current: false, best: false, friends_only: true, maintenance: null },
    ],
  },
  find_on_network: [],
  // ?whatsnew: What's new after an update in the preview.
  whats_new: location.search.includes('whatsnew')
    ? {
      from: '0.4.6', to: '0.4.8', headlines: 3, notes: ['All release notes', 'https://github.com/JDevWebb/5th-echelon-enhanced/releases'],
      releases: [
        { version: '0.4.8', thanks: 'Tank, Nova and Raven', new: [{ title: 'A new launcher', text: 'Field ops: a new look, smoother and quicker.', community: false }, { title: 'Global matchmaking', text: 'Find a match across every community server.', community: true }], fixed: [['Joins no longer time out', ' on a far server.']] },
        { version: '0.4.7', thanks: null, new: [{ title: 'Chat answers straight away again', text: '', community: true }], fixed: [] },
      ],
    }
    : null,
  // ?feedback: the ask after a game in the preview.
  feedback: location.search.includes('feedback')
    ? {
      why: 'The game seems to have crashed.', mismatch: null,
      problems: [['join', 'Couldn’t join a match or a friend'], ['lag', 'Lag, or things out of sync'], ['connection', 'Lost the connection'], ['crash', 'The game crashed or froze'], ['version', '"Different version of the game"'], ['signin', 'Couldn’t sign in'], ['other', 'Something else']],
      ticked: ['crash'], files: [['bl-tracing.log', 412], ['launcher.log', 18], ['checklist.txt', 1]], attach: true, manual: false, server: 'Falkenstein, Germany',
    }
    : null,
  feedback_file: '[20:14:02] WARN net: connection to host lost\n[20:14:03] ERROR game: unhandled exception',
  // ?notice: a release going out in the preview.
  notice: location.search.includes('notice')
    ? {
      heading: 'Update 0.4.8 going out', eta: '8:40 pm', text: 'Your server updates at about 8:40 pm and is back a couple of minutes later. Sydney already runs it: play there meanwhile.',
      note: null, play_on: ['oceania.scbl.jdevwebb.net', 'Play on Sydney'],
      servers: [
        { place: 'Falkenstein, Germany', mine: true, version: '0.4.7', doing: 'Updates at about 8:40 pm', meter: null, tone: 'waiting' },
        { place: 'Beauharnois, Canada', mine: false, version: '0.4.7 → 0.4.8', doing: 'Installing and restarting', meter: 0.6, tone: 'now' },
        { place: 'Sydney, Australia', mine: false, version: '0.4.8', doing: 'Updated', meter: null, tone: 'done' },
      ],
      status: 'Your server updates at 8:40 pm', blocks_play: null, every: 60,
    }
    : null,
  squad: {
    online: [
      { id: '1', name: 'Tank', activity: 'SvM Classic · Penthouse · 3 of 4', joinable: true },
      { id: '2', name: 'Nova', activity: null, joinable: false },
    ],
    elsewhere: [{ name: 'Raven', place: 'Sydney, Australia', host: 'oceania.scbl.jdevwebb.net', public: true }],
    others: [{ id: '9', name: 'Ghost', activity: 'In the lobby', asked: false, asked_us: false }],
    offline: 5, requests: 1, join_offered: true, overlay_key: 'F1', name_not_allowed: null, managed: false,
    playing: ['SvM Classic · Penthouse · 3/4', 'Co-op · Dead Drop · 2/4'], playing_more: 'Also: 3 in lobbies',
  },
  news: [
    { title: '0.4.7 "Clear Channel"', text: 'Chat, friends and sign-in answer straight away again on busy servers.', link: '' },
    { title: '0.4.6 "Comms Check"', text: 'Chat is here: Global, Friends and your lobby, across the community network.', link: '' },
  ],
  checks: {
    checks: [
      { id: 'client', status: 'ok', title: 'The 5th Echelon client is installed', detail: '', fix: null },
      { id: 'account', status: 'ok', title: 'Signed in as Kiwi', detail: '', fix: null },
      { id: 'network', status: 'warn', title: 'Unencrypted connection', detail: 'Sign-ins travel readable to this server; ask its operator to set up HTTPS.', fix: null },
      { id: 'save', status: 'fail', title: 'No save for this account', detail: 'Online play needs a save at rank 5 or more.', fix: ['create-save', 'Set up a save'] },
    ],
    support: null,
    tone: 'fail',
    line: '1 thing to fix',
    fixable: true,
    busy: false,
  },
};
// The preview with the first-run questions answered (?answered).
if (location.search.includes('answered')) {
  sample.questions.diagnostics = null;
  sample.questions.options = [];
}
// The preview starts as a first run (?first): no server yet.
if (location.search.includes('first')) {
  sample.info.server = null;
  sample.info.account = null;
}
// What the browser preview's commands do to its made-up state.
const preview = {
  play: () => {
    sample.activity = { id: 1, title: 'Starting the game', step: null, state: 'done', message: null, progress: null, steps: [], waiting: 0 };
    setTimeout(() => (sample.activity = null), 4000);
    return true;
  },
  answer_diagnostics: () => { sample.questions.diagnostics = null; },
  answer_options: () => { sample.questions.options = []; },
  set_play_mode: ({ mode }) => { sample.info.play_mode = mode; },
  dismiss: () => { sample.activity = null; },
  choose_game: () => true,
  pick_game_folder: () => false,
  activity_action: () => null,
  fix: () => {
    sample.activity = { id: 2, title: 'Fixing your save', step: 'Raising the save to rank 5', state: 'working', message: null, progress: null, steps: [], actions: [], waiting: 0 };
    previewEmit('activity');
    setTimeout(() => {
      sample.activity = { id: 2, title: 'Your save is set up', step: null, state: 'done', message: null, progress: null, steps: [], actions: [], waiting: 0 };
      sample.checks.checks[3] = { id: 'save', status: 'ok', title: 'Save at rank 5', detail: '', fix: null };
      Object.assign(sample.checks, { tone: 'warn', line: 'Ready to play · 1 note', fixable: false });
      previewEmit('activity');
      previewEmit('checks');
    }, 1500);
    return true;
  },
  fix_everything: () => true,
  switch_server: () => null,
  join_friend: () => null,
  whats_new_seen: () => null,
  open_url: () => null,
  key_art: () => null,
  dismiss_emulator_notice: () => null,
  ask_name_review: () => null,
  account_server: ({ server, own }) => (own
    ? { place: server, kind: 'A server of your own', ping: null, own: true, current: 'Falkenstein, Germany', leaving: 'the community network' }
    : { place: 'Sydney, Australia', kind: '5th Echelon Community Oceania · community server', ping: 290, own: false, current: 'Falkenstein, Germany', leaving: 'the community network' }),
  account_probe: () => ({ api: 'https://oceania.scbl.jdevwebb.net', version: '0.4.7', ping: 290 }),
  check_name: ({ name }) => (name.toLowerCase() === 'kiwi' ? { kind: 'taken' } : { kind: 'free' }),
  name_suggestions: () => ['KiwiNZ', 'Kiwi2'],
  recent_servers: () => [{ address: '192.168.1.20', label: 'LAN party', when: 'used 2 days ago' }],
  usual_name: () => 'Kiwi',
  use_network: () => null,
  use_community_network: () => null,
  roadmap: () => ({
    as_of: null, error: null,
    lanes: [
      { id: 'shipping', title: 'Shipping', release: '0.4.8', items: [{ id: 1, title: 'The launcher in Tauri', body: 'Field ops, smoother and quicker.', tags: ['launcher'], status: 'In testing' }, { id: 2, title: 'Global matchmaking', body: 'One search across every community server.', tags: [], status: 'Building' }] },
      { id: 'next', title: 'Next', release: '0.4.9', items: [{ id: 3, title: 'Lobby browser', body: '', tags: ['matches'], status: '' }] },
      { id: 'later', title: 'Later', release: '', items: [] },
      { id: 'requested', title: 'Requested', release: '', items: [{ id: 4, title: 'Custom maps', body: 'Brand-new Spies vs Mercs maps.', tags: [], status: '' }] },
    ],
  }),
  suggestion_form: () => ({ areas: ['Launcher', 'In the game', 'Matches and joining', 'Servers', 'Other'], title_max: 80, text_max: 1000, identity: true }),
  check_suggestion: ({ title }) => (title.trim() ? null : 'Give it a title.'),
  suggest: () => null,
  settings: () => ({
    sections: [['game', 'Game'], ['network', 'Network'], ['servers', 'Servers and accounts'], ['identity', 'Identity and friends'], ['notifications', 'Notifications'], ['save', 'Save game'], ['client', '5th Echelon client'], ['advanced', 'Advanced'], ['display', 'Display'], ['feedback', 'Feedback'], ['about', 'About']],
    managed: null,
    game: {
      dir: 'C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM', renderer: 'dx11', through_steam: false, proton: null, overlay: true, overlay_key: 'F1',
      overlay_keys: ['F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12', 'Insert', 'Home'], auto_join_invite: true, mouse_accel_blocked: null,
      disable_mouse_accel: true, brighten: true, brightness: 140, brightness_range: [100, 220], disable_autoclimb: false, log_level: 'Info',
      log_levels: ['Error', 'Warning', 'Info', 'Debug', 'Trace'], command_line: '', running: false, mouse_turn_per_second: false,
      beta_features: [['Mouse turning the same at any frame rate', 'Above 60 fps the game turns less for the same mouse movement.']],
    },
    network: { nat: 'auto', port_mapping: true, adapters: [['Ethernet', '192.168.1.20'], ['Wi-Fi', '192.168.1.31']], adapter: null, require_adapter: false, automatic: 'Automatic (the one that reaches the server)' },
    notifications: ['outside-matches', 'always', 'always'],
    toasts: [['never', 'Never'], ['only-mentions', 'Only when they name me'], ['outside-matches', 'Outside matches'], ['always', 'Always']],
    profiles: [{ name: 'eu1', server: 'eu1.scbl.jdevwebb.net', username: 'Kiwi', current: true }, { name: 'lan', server: '192.168.1.20', username: 'Kiwi', current: false }],
    save: { path: 'C:\\Users\\Kiwi\\Documents\\5th Echelon\\1.save', no_folder: null, state: '412 300 XP', missing: false, ubisoft: null },
    client: { state: 'Installed.', version: '0.4.8-dev', can_install: false, can_uninstall: true },
    hooks_all: true, hooks: [['Printer', 'Printer', false], ['Overlay', 'Overlay', true], ['Networking', 'Networking', true]], log_data_version: false,
    send_diagnostics: true, ask_feedback: true, directory: 'https://scbl.jdevwebb.net', size: 1, size_range: [0.7, 1.6], product: '5th Echelon Enhanced', release: '0.4.8-dev',
  }),
  set: () => null,
  act: () => null,
  identity: () => ({ short: 'DNIC-JULS', error: null, accounts: 2 }),
  my_reports: () => [{ line: '28 Oct · Falkenstein · open', said: '“froze joining a friend”', reply: '' }],
  connection_test: () => [['Config server (port 80)', 'ok', null], ['API and account', 'ok', null], ['How players reach this PC', 'ok', 'Direct: reachable without any router set-up'], ['Internet play helper', 'warn', 'reachable; matches go through the server’s relay']],
  check_update: () => ['0.4.7', false],
  choose_match_offered: () => true,
  relays: () => (location.search.includes('relays')
    ? { list: [{ id: 'na1', name: 'North America', region: 'Canada', ms: 142 }, { id: 'syd', name: 'Sydney', region: 'Oceania', ms: 28 }, { id: 'eu1', name: 'Europe', region: 'Germany', ms: null }], in_use: 'syd', chosen: null }
    : { list: [] }),
  choose_relay: () => null,
  matches: () => (location.search.includes('nomatches')
    ? { offered: true, answered: true, rows: [], others: '' }
    : {
      offered: true, answered: true, others: '1 private match · 3 in lobbies',
      rows: [
        { room: 4375, mode: 'SvM Classic', map: 'Virus Vault', players: '6/8', started: true, full: false },
        { room: 4391, mode: 'SvM Blacklist', map: 'Penthouse', players: '3/8', started: false, full: false },
        { room: 4402, mode: 'Co-op', map: 'Billionaire’s Yacht', players: '2/2', started: false, full: true },
        { room: 4410, mode: 'Grim (Ghost)', map: null, players: '1/4', started: false, full: false },
      ],
    }),
  join_match: () => null,
  new_match: () => null,
  cancel_new_match: () => null,
  update_banner: () => (location.search.includes('update') ? { version: '0.4.9', page: 'https://github.com/JDevWebb/5th-echelon-enhanced/releases', updating: false } : null),
  update_later: () => null,
  update_now: () => null,
  update_asked: () => null,
  host: () => ({
    exe_name: 'dedicated_server.exe', found: true, error: null, running: false, starting: false, downloading: false, listen: null,
    adapters: [['Ethernet', '192.168.1.20']], trusted_subnet: '', api: { info: true, presence: true, accounts: false, unhandled: false }, managing: null, local: false,
  }),
  host_logs: () => [{ time: '20:14:02', tone: '', msg: 'Listening on 0.0.0.0:21126' }, { time: '20:14:09', tone: 'warn', msg: 'A client sent an unknown call' }],
  chat_glance: () => ({ unread: 2, latest: ['Raven', '@Kiwi you hosting tonight?'] }),
  chat: ({ tab, with: w }) => ({
    state: 'open', why: null, trouble: null, read_only: false, may_post_global: true, global_wait: '', lobby: false, unread: 2,
    rules: 'Messages are kept for 7 days and the admins can read them. No links, keep it friendly.', max_message: 500,
    with_name: w ? 'Tank' : null,
    messages: tab === 'global'
      ? [
        { id: 1, time: '20:01', from: 'a', from_name: 'Raven', server: 'Sydney', parts: [{ text: '@Kiwi you hosting tonight?', link: false }], mine: false, mentions_me: true, friend: true },
        { id: 2, time: '20:02', from: 'me', from_name: 'Kiwi', server: '', parts: [{ text: 'Yep, Penthouse at 8', link: false }], mine: true, mentions_me: false, friend: false },
        { id: 3, time: '20:04', from: 'b', from_name: 'Ghost', server: 'EU', parts: [{ text: 'Join us: ', link: false }, { text: 'discord.gg/scbl', link: true }], mine: false, mentions_me: false, friend: false },
      ]
      : [],
    conversations: [{ with: 't', name: 'Tank', what: 'Direct', unread: 2 }, { with: 'n', name: 'Nova', what: 'Friend · online', unread: 0 }],
    online: [{ id: '1', name: 'Tank', activity: 'SvM Classic', friend: true, asked: false }, { id: '9', name: 'Ghost', activity: null, friend: false, asked: false }],
  }),
  chat_check: ({ text }) => ({ why: /https?:\/\//.test(text) ? 'Links aren’t allowed in chat (a Discord invite is).' : null, masked: false }),
  chat_send: () => null,
  support: () => ({
    identity: true, error: null, resolved: false, unread: 0, max_text: 2000, draft: null, has_game: true, sending: false,
    messages: [
      { admin: false, who: 'You', when: '28 Oct 20:14', text: 'My game froze joining a friend on Sydney.', files: 'Sent with bl-tracing.log, launcher.log' },
      { admin: true, who: 'nexusx44', when: '28 Oct 21:02', text: 'Thanks: fixed in 0.4.7. Update and try again?', files: null },
    ],
    manual: [['Troubleshooting', 'https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Troubleshooting'], ['Error messages', 'https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Error-Messages'], ['Antivirus warnings', 'https://github.com/JDevWebb/5th-echelon-enhanced/wiki/Antivirus-Warnings']],
  }),
  support_check: ({ text }) => (text.trim() ? null : 'Write your message first.'),
  support_send: () => null,
  ask_feedback: () => null,
  my_suggestions: () => [{ title: 'Voice chat', line: 'In the game · 3 Oct', status: 'Planned', reply: 'On the list for after 0.4.8.' }],
  feedback_send: () => { sample.feedback = null; },
  feedback_close: () => { sample.feedback = null; },
  player_action: () => null,
  switch_to_friend: ({ host }) => {
    setTimeout(() => previewEmit('setup', { kind: 'needs-name', server: host, suggested: 'Kiwi', taken: false }), 600);
    return true;
  },
  identify: () => null,
  connect: ({ server, newName }) => {
    sample.info.connecting = true;
    setTimeout(() => {
      sample.info.connecting = false;
      if (newName) {
        sample.info.server = server;
        sample.info.account = newName;
        previewEmit('setup', { kind: 'ready', server });
      } else {
        previewEmit('setup', { kind: 'needs-name', server, suggested: 'Kiwi', taken: false });
      }
    }, 900);
    return true;
  },
};

/** Calls a Rust command (`name`, with `args`): its answer. */
export async function call(name, args = {}) {
  if (tauri) return tauri.core.invoke(name, args);
  if (preview[name]) return preview[name](args);
  return structuredClone(sample[name]);
}

const previewListeners = {};
function previewEmit(event, payload) {
  for (const h of previewListeners[event] ?? []) h({ payload });
}

/** Calls `handler` on each `event` from the Rust side; answers how to stop. */
export async function listen(event, handler) {
  if (tauri) return tauri.event.listen(event, handler);
  (previewListeners[event] ??= []).push(handler);
  return () => (previewListeners[event] = previewListeners[event].filter((h) => h !== handler));
}

/** The window's own buttons. */
export const win = {
  minimize: () => tauri?.window.getCurrentWindow().minimize(),
  toggleMaximize: () => tauri?.window.getCurrentWindow().toggleMaximize(),
  close: () => tauri?.window.getCurrentWindow().close(),
};
