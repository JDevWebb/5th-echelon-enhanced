// The way to the Rust side (src-tauri/src/commands.rs) and to the window. In a plain browser
// (`npm run dev`, or a preview), made-up answers instead, so the screens can be looked at alone.
const tauri = window.__TAURI__;
export const inTauri = Boolean(tauri);

const sample = {
  info: {
    product: '5th Echelon Enhanced', release: '0.4.8-dev', game_dir: 'C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM',
    server: 'eu1.scbl.jdevwebb.net', account: 'Kiwi', community: true, play_mode: 'play', managed: false, running: false,
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
};
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
};

/** Calls a Rust command (`name`, with `args`): its answer. */
export async function call(name, args = {}) {
  if (tauri) return tauri.core.invoke(name, args);
  if (preview[name]) return preview[name](args);
  return structuredClone(sample[name]);
}

/** Calls `handler` on each `event` from the Rust side; answers how to stop. */
export async function listen(event, handler) {
  if (tauri) return tauri.event.listen(event, handler);
  return () => {};
}

/** The window's own buttons. */
export const win = {
  minimize: () => tauri?.window.getCurrentWindow().minimize(),
  toggleMaximize: () => tauri?.window.getCurrentWindow().toggleMaximize(),
  close: () => tauri?.window.getCurrentWindow().close(),
};
