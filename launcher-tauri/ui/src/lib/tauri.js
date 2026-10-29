// The way to the Rust side (src-tauri/src/commands.rs) and to the window. In a plain browser
// (`npm run dev`, or a preview), made-up answers instead, so the screens can be looked at alone.
const tauri = window.__TAURI__;
export const inTauri = Boolean(tauri);

const SAMPLE = {
  info: {
    product: '5th Echelon Enhanced', release: '0.4.8-dev', game_dir: 'C:\\Games\\Splinter Cell Blacklist\\src\\SYSTEM',
    server: 'eu1.scbl.jdevwebb.net', account: 'Kiwi', community: true,
  },
};

/** Calls a Rust command (`name`, with `args`): its answer. */
export async function call(name, args = {}) {
  if (tauri) return tauri.core.invoke(name, args);
  return structuredClone(SAMPLE[name]);
}

/** The window's own buttons. */
export const win = {
  minimize: () => tauri?.window.getCurrentWindow().minimize(),
  toggleMaximize: () => tauri?.window.getCurrentWindow().toggleMaximize(),
  close: () => tauri?.window.getCurrentWindow().close(),
};
