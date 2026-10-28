// The Tauri trial's page: its data from the Rust side (`snapshot`), the window's own buttons, the
// drawer's tabs, the server menu and the turning news. In a plain browser (no Tauri), it shows
// the same made-up data, so the page can be looked at on its own.
const tauri = window.__TAURI__;
const sample = {
  player: 'Kiwi', server: 'Falkenstein, Germany', region: 'eu1.scbl.jdevwebb.net', ping_ms: 38, online: 24,
  ready: 'Ready to play · 1 note',
  checks: [{ title: 'Unencrypted connection', detail: 'Sign-ins travel readable to this server; ask its operator to set up HTTPS.', fix: null }],
  friends: [
    { name: 'Tank', activity: 'SvM Classic · Penthouse · 3 of 4', joinable: true },
    { name: 'Nova', activity: 'In the lobby · Sydney', joinable: true },
    { name: 'Raven', activity: 'Launcher open', joinable: false },
  ],
  news: ['0.4.7: chat, friends and sign-in answer straight away again on busy servers.', '0.4.6 "Comms Check": chat is here.', "The community network's new address."],
  latest_global: ['Raven', '@Kiwi you hosting tonight?'],
  started_ms: 0,
};

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

async function load() {
  const s = tauri ? await tauri.core.invoke('snapshot') : sample;
  $('player').textContent = s.player;
  $('server-name').textContent = s.server;
  $('ping').textContent = `${s.ping_ms} ms`;
  $('online').textContent = `${s.online} online`;
  $('ready').textContent = s.ready;
  $('started').textContent = tauri ? `SHOWN ${Math.round(performance.now())} MS AFTER THE PAGE, ${s.started_ms} MS AFTER START` : 'BROWSER PREVIEW';
  for (const c of s.checks) {
    const li = el('li');
    li.append(el('span', 'edge busy'));
    const t = el('span'); t.append(el('b', null, c.title), el('small', null, c.detail)); li.append(t);
    $('checks').append(li);
  }
  for (const f of s.friends) {
    const li = el('li');
    li.append(el('span', `edge${f.activity.startsWith('SvM') ? ' busy' : ''}`));
    const t = el('span'); t.append(el('b', null, f.name), el('small', null, f.activity.toUpperCase())); li.append(t);
    li.append(el('button', f.joinable ? '' : 'quiet', f.joinable ? 'Join' : 'Message'));
    $('friends').append(li);
  }
  for (const n of s.news) {
    const li = el('li'); const t = el('span'); t.append(el('b', null, n)); li.append(t); $('briefings').append(li);
  }
  const latest = $('latest');
  latest.append(el('b', null, s.latest_global[0]), document.createTextNode(` ${s.latest_global[1]}`));
  // The news along the foot turns every few seconds, fading across.
  let i = 0;
  const news = $('news');
  news.textContent = s.news[0];
  setInterval(() => {
    news.style.opacity = 0;
    setTimeout(() => { i = (i + 1) % s.news.length; news.textContent = s.news[i]; news.style.opacity = 1; }, 400);
  }, 5000);
}

// The window's own title bar.
if (tauri) {
  const win = tauri.window.getCurrentWindow();
  $('min').onclick = () => win.minimize();
  $('max').onclick = () => win.toggleMaximize();
  $('close').onclick = () => win.close();
}

// The drawer's tabs: the underline slides to the one chosen, its pane fades in.
const tabs = [...document.querySelectorAll('.tabs button')];
const underline = document.querySelector('.underline');
function pick(tab) {
  const at = tabs.indexOf(tab);
  tabs.forEach((t) => t.classList.toggle('on', t === tab));
  underline.style.transform = `translateX(${at * 100}%)`;
  document.querySelectorAll('.pane').forEach((p) => p.classList.toggle('on', p.dataset.pane === tab.dataset.tab));
}
tabs.forEach((t) => (t.onclick = () => pick(t)));
pick(tabs.find((t) => t.classList.contains('on')));

// The server menu: opens over the stage, closes on a click elsewhere or Escape.
const server = $('server');
const menu = $('menu');
server.onclick = (e) => { e.stopPropagation(); menu.classList.toggle('open'); server.classList.toggle('open'); };
document.addEventListener('click', () => { menu.classList.remove('open'); server.classList.remove('open'); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') { menu.classList.remove('open'); server.classList.remove('open'); } });

load();
