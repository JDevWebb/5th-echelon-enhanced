// The way to the Rust side (src-tauri/src/commands.rs) and to the window. In a plain browser
// (`npm run dev`, or a preview), made-up answers instead, so the screens can be looked at alone.
const tauri = window.__TAURI__;
export const inTauri = Boolean(tauri);

const sample = {
  info: {
    product: '5th Echelon Enhanced', release: '0.4.8-dev', game_dir: 'C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM',
    server: 'eu1.scbl.jdevwebb.net', account: 'Kiwi', community: true, play_mode: 'play', managed: false, running: false,
    connecting: false, max_name: 24,
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
    card: { caption: 'Server', caption_why: null, place: 'Falkenstein, Germany', ping: 38, players: 24 },
    note: null,
    rows: [
      { host: 'eu1.scbl.jdevwebb.net', place: 'Falkenstein, Germany', name: 'EU', title: '5th Echelon Community EU (Falkenstein, Germany)', ping: 38, players: 24, playing: 'SvM Classic 6/8 · 3 in lobbies', current: true, best: true, friends_only: false, maintenance: null },
      { host: 'na1.scbl.jdevwebb.net', place: 'Beauharnois, Canada', name: 'North America', title: '5th Echelon Community North America (Beauharnois, Canada)', ping: 112, players: 9, playing: '', current: false, best: false, friends_only: true, maintenance: null },
      { host: 'oceania.scbl.jdevwebb.net', place: 'Sydney, Australia', name: 'Oceania', title: '5th Echelon Community Oceania (Sydney, Australia)', ping: 290, players: 4, playing: '', current: false, best: false, friends_only: true, maintenance: null },
    ],
  },
  find_on_network: [],
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
