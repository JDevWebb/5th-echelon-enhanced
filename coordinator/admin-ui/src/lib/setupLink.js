// A setup link (#setup=…), taken out of the address bar before the router sees it. The router
// is made as its module loads and rewrites the hash at once (to #/setup=…, then to a page it
// knows), so this module must load first: main.js imports it before anything else.
let token = null;
const listeners = [];

function take() {
  const m = location.hash.match(/^#\/?setup=([A-Za-z0-9]+)$/);
  if (!m) return false;
  // Out of the address bar and history at once.
  history.replaceState(null, '', location.pathname);
  token = m[1];
  return true;
}

take();
// A link opened in a tab already showing the admin UI only changes the hash: popstate comes
// before hashchange, and this listener before the router's (added as its module loads, later).
window.addEventListener('popstate', () => {
  if (take()) listeners.forEach(f => f());
});

/** The setup link's token, once. */
export function setupToken() {
  const t = token;
  token = null;
  return t;
}

/** Calls `f` when a setup link is opened in this tab. */
export function onSetupLink(f) {
  listeners.push(f);
}
