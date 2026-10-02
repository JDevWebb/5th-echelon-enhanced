// SCBL Network Ops: the coordinator's admin UI. No dependencies: everything
// here is served by the coordinator itself (see admin/mod.rs).

import { LAND } from './world.js';

// ---------------------------------------------------------------- helpers

const app = document.getElementById('app');
const SVG = 'http://www.w3.org/2000/svg';

/** Builds an element: h('div.panel#id', {attrs}, ...children). Text is never parsed as HTML. */
function h(spec, attrs, ...children) {
  const m = spec.match(/^([a-z0-9]+)((?:[.#][\w-]+)*)$/i);
  const el = document.createElement(m ? m[1] : 'div');
  if (m) for (const part of m[2].match(/[.#][\w-]+/g) || []) {
    if (part[0] === '.') el.classList.add(part.slice(1)); else el.id = part.slice(1);
  }
  if (attrs && (typeof attrs !== 'object' || attrs instanceof Node || Array.isArray(attrs))) { children.unshift(attrs); attrs = null; }
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'class') el.className += ' ' + v;
    // Through the DOM: the page's CSP refuses style attributes.
    else if (k === 'style') el.style.cssText = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  add(el, children);
  return el;
}
function add(el, children) {
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}
function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) if (v != null) { if (k === 'style') el.style.cssText = v; else el.setAttribute(k, v); }
  for (const c of children.flat()) if (c != null) el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  return el;
}

const fmt = {
  n: (v, d = 0) => v == null || Number.isNaN(v) ? '–' : Number(v).toLocaleString(undefined, { maximumFractionDigits: d, minimumFractionDigits: d }),
  pct: v => v == null ? '–' : `${Math.round(v)}%`,
  bytes(v) {
    if (v == null) return '–';
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (Math.abs(v) >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return `${v.toFixed(v < 10 && i ? 1 : 0)} ${u[i]}`;
  },
  rate: v => v == null ? '–' : `${fmt.bits(v * 8)}`,
  bits(v) {
    const u = ['bps', 'kbps', 'Mbps', 'Gbps'];
    let i = 0;
    while (Math.abs(v) >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    return `${v.toFixed(v < 10 && i ? 1 : 0)} ${u[i]}`;
  },
  ms: v => v == null ? '–' : `${Math.round(v)} ms`,
  ago(t) {
    if (!t) return 'never';
    const d = Math.max(0, Date.now() / 1000 - t);
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)} min ago`;
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
    return `${Math.floor(d / 86400)} d ago`;
  },
  dur(secs) {
    if (!secs) return '–';
    const d = Math.floor(secs / 86400), hr = Math.floor(secs % 86400 / 3600), m = Math.floor(secs % 3600 / 60);
    return d ? `${d}d ${hr}h` : hr ? `${hr}h ${m}m` : `${m}m`;
  },
  when: t => t ? new Date(t * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '–',
};

function toast(msg, bad = false) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.className = bad ? 'bad' : '';
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { t.hidden = true; }, 4200);
}

// ---------------------------------------------------------------- API

class ApiError extends Error {
  constructor(status, body) { super(body.error || `HTTP ${status}`); this.status = status; this.body = body; }
}

async function call(method, path, body) {
  const resp = await fetch('/api' + path, {
    method,
    credentials: 'same-origin',
    headers: { 'X-FES-Admin': '1', ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const data = await resp.json().catch(() => ({}));
  if (!resp.ok) throw new ApiError(resp.status, data);
  return data;
}

/** An API call from a signed-in page: asks to confirm it's them when the server wants a fresh
 * second factor, and goes back to signing in when the session ended. */
async function api(method, path, body) {
  try {
    return await call(method, path, body);
  } catch (e) {
    if (e.status === 403 && e.body?.reverify) {
      if (await reverify()) return call(method, path, body);
      throw new Error('Not confirmed.');
    }
    if (e.status === 401) { state.session = null; boot(); }
    throw e;
  }
}

// ---------------------------------------------------------------- passkeys

const b64u = {
  toBuf(s) {
    const pad = '='.repeat((4 - s.length % 4) % 4);
    const bin = atob((s + pad).replace(/-/g, '+').replace(/_/g, '/'));
    return Uint8Array.from(bin, c => c.charCodeAt(0)).buffer;
  },
  from(buf) {
    let bin = '';
    for (const b of new Uint8Array(buf)) bin += String.fromCharCode(b);
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  },
};

const passkeysWork = () => !!(window.PublicKeyCredential && navigator.credentials);

async function passkeyGet(options) {
  const publicKey = {
    ...options,
    challenge: b64u.toBuf(options.challenge),
    allowCredentials: (options.allowCredentials || []).map(c => ({ ...c, id: b64u.toBuf(c.id) })),
  };
  const cred = await navigator.credentials.get({ publicKey });
  const r = cred.response;
  return {
    id: cred.id, type: cred.type,
    response: {
      clientDataJSON: b64u.from(r.clientDataJSON),
      authenticatorData: b64u.from(r.authenticatorData),
      signature: b64u.from(r.signature),
      userHandle: r.userHandle ? b64u.from(r.userHandle) : null,
    },
  };
}

async function passkeyCreate(options) {
  const publicKey = {
    ...options,
    challenge: b64u.toBuf(options.challenge),
    user: { ...options.user, id: b64u.toBuf(options.user.id) },
    excludeCredentials: (options.excludeCredentials || []).map(c => ({ ...c, id: b64u.toBuf(c.id) })),
  };
  const cred = await navigator.credentials.create({ publicKey });
  return {
    id: cred.id, type: cred.type,
    response: { clientDataJSON: b64u.from(cred.response.clientDataJSON), attestationObject: b64u.from(cred.response.attestationObject) },
  };
}

/** Signs in (or confirms it's them) with a passkey. */
async function usePasskey() {
  const begin = await call('POST', '/login/passkey/begin', {});
  const credential = await passkeyGet(begin.options);
  return call('POST', '/login/passkey/finish', { challenge_id: begin.challenge_id, credential });
}

function passkeyError(e) {
  if (e?.name === 'NotAllowedError') return 'The passkey prompt was cancelled or timed out.';
  if (e?.name === 'InvalidStateError') return 'That passkey is registered already.';
  return e?.message || String(e);
}

// ---------------------------------------------------------------- state and routing

const state = { session: null, overview: null, timer: null, range: {} };

const ROUTES = [
  ['overview', 'Overview', 'M3 13h4v8H3zM10 3h4v18h-4zM17 9h4v12h-4z'],
  ['servers', 'Servers', 'M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01'],
  ['players', 'Players & map', 'M12 21s-7-6.2-7-11a7 7 0 1 1 14 0c0 4.8-7 11-7 11zM12 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z'],
  ['activity', 'Playlists', 'M4 6h16M4 12h10M4 18h13'],
  ['network', 'Network', 'M2 12h4l3-8 4 16 3-8h6'],
  ['updates', 'Updates', 'M12 3v12m0 0-4-4m4 4 4-4M5 21h14'],
  ['security', 'Security', 'M12 3 4 6v6c0 5 3.4 8.2 8 9 4.6-.8 8-4 8-9V6z'],
  ['audit', 'Audit log', 'M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01'],
];

function route() {
  const [name, arg] = location.hash.replace(/^#\/?/, '').split('/');
  return { name: ROUTES.some(r => r[0] === name) ? name : 'overview', arg };
}

window.addEventListener('hashchange', () => { if (state.session?.stage === 'full') render(); });
document.addEventListener('visibilitychange', () => { if (!document.hidden && state.session?.stage === 'full' && route().name === 'overview') render(); });

async function boot() {
  clearInterval(state.timer);
  const setup = location.hash.match(/^#setup=([A-Za-z0-9]+)$/);
  if (setup) {
    // Out of the address bar and history at once.
    history.replaceState(null, '', location.pathname);
    return setupView(setup[1]);
  }
  try {
    state.session = await call('GET', '/session');
  } catch (e) {
    state.session = null;
    return loginView(null, e.body?.you);
  }
  if (state.session.stage === 'password') return secondFactorView();
  if (state.session.stage === 'enroll') return enrollView();
  render();
}

// ---------------------------------------------------------------- sign-in

function brand(sub = 'Network operations') {
  return h('div.brand', h('span.goggles', h('i'), h('i'), h('i')), h('div', h('b', 'SCBL Network'), h('span', sub)));
}

function gate(...content) {
  app.replaceChildren(h('div.gate', h('div.card', ...content)));
}

function where(you) {
  return you ? h('p.where', `Connecting from ${you.ip}${you.country ? ' · ' + you.country : ''}`) : null;
}

function loginView(error, you) {
  const err = h('p.err', { role: 'alert' }, error || '');
  const user = h('input', { name: 'username', autocomplete: 'username webauthn', required: true, autofocus: true });
  const pass = h('input', { name: 'password', type: 'password', autocomplete: 'current-password', required: true });
  const submit = h('button.primary', { type: 'submit' }, 'Sign in');
  const form = h('form.stack', {
    async onsubmit(ev) {
      ev.preventDefault();
      submit.disabled = true;
      err.textContent = '';
      try {
        const r = await call('POST', '/login', { username: user.value.trim(), password: pass.value });
        state.session = { stage: r.stage, totp: r.totp, passkeys: r.passkeys };
        r.stage === 'enroll' ? enrollView() : secondFactorView();
      } catch (e) {
        err.textContent = e.message;
        submit.disabled = false;
        pass.select();
      }
    },
  },
  h('label.field', h('span', 'Admin name'), user),
  h('label.field', h('span', 'Password'), pass),
  submit);
  const passkey = passkeysWork() ? h('button', {
    type: 'button',
    async onclick() {
      err.textContent = '';
      try { await usePasskey(); boot(); } catch (e) { err.textContent = passkeyError(e.body ? e : e); }
    },
  }, keyIcon(), 'Sign in with a passkey') : null;
  gate(brand(), h('h1', 'Sign in'), form, passkey && h('div.divider', 'or'), passkey, err, where(you));
}

function keyIcon() {
  return s('svg', { viewBox: '0 0 24 24', width: 16, height: 16, fill: 'none', stroke: 'currentColor', 'stroke-width': 2, 'stroke-linecap': 'round' },
    s('circle', { cx: 8, cy: 15, r: 4 }), s('path', { d: 'M10.8 12.2 20 3m-3 3 3 3m-6 0 2 2' }));
}

function codeInput(placeholder = '000000') {
  return h('input.code', { inputmode: 'numeric', autocomplete: 'one-time-code', maxlength: 7, placeholder, pattern: '[0-9 ]*' });
}

function secondFactorView() {
  const ses = state.session || {};
  const err = h('p.err', { role: 'alert' });
  const done = () => boot();
  const parts = [];
  if (ses.passkeys > 0 && passkeysWork()) {
    parts.push(h('button.primary', {
      async onclick() {
        err.textContent = '';
        try { await usePasskey(); done(); } catch (e) { err.textContent = passkeyError(e); }
      },
    }, keyIcon(), 'Use your passkey'));
  }
  if (ses.totp) {
    if (parts.length) parts.push(h('div.divider', 'or'));
    const code = codeInput();
    parts.push(h('form.stack', {
      async onsubmit(ev) {
        ev.preventDefault();
        err.textContent = '';
        try { await call('POST', '/login/totp', { code: code.value }); done(); } catch (e) { err.textContent = e.message; code.select(); if (e.status === 401 && /again/.test(e.message)) boot(); }
      },
    }, h('label.field', h('span', 'Code from your authenticator app'), code), h('button', { type: 'submit' }, 'Verify')));
    setTimeout(() => code.focus());
  }
  const recovery = h('form.stack', {
    hidden: true,
    async onsubmit(ev) {
      ev.preventDefault();
      err.textContent = '';
      try { await call('POST', '/login/recovery', { code: recovery.querySelector('input').value }); done(); } catch (e) { err.textContent = e.message; }
    },
  }, h('label.field', h('span', 'Recovery code'), h('input', { autocomplete: 'off', placeholder: 'xxxx-xxxx-xxxx' })), h('button', { type: 'submit' }, 'Use recovery code'));
  gate(brand(), h('h1', 'Confirm it\'s you'), h('p.muted', 'Your password was right. Now your second factor.'), ...parts,
    h('button.ghost.small', { type: 'button', onclick() { recovery.hidden = !recovery.hidden; } }, 'Lost your device? Use a recovery code'),
    recovery, err,
    h('button.ghost.small', { type: 'button', async onclick() { await call('POST', '/logout', {}); boot(); } }, 'Start over'));
}

function setupView(token) {
  const err = h('p.err', { role: 'alert' });
  const pass = h('input', { type: 'password', autocomplete: 'new-password', minlength: 12, required: true, autofocus: true });
  const again = h('input', { type: 'password', autocomplete: 'new-password', required: true });
  gate(brand('Admin setup'), h('h1', 'Set up your sign-in'),
    h('p.muted', 'Choose a password (12 characters or more; a passphrase is best). Next you\'ll add a passkey or an authenticator app.'),
    h('form.stack', {
      async onsubmit(ev) {
        ev.preventDefault();
        err.textContent = '';
        if (pass.value !== again.value) { err.textContent = 'The passwords don\'t match.'; return; }
        try {
          await call('POST', '/setup', { token, password: pass.value });
          state.session = { stage: 'enroll' };
          enrollView();
        } catch (e) { err.textContent = e.message; }
      },
    }, h('label.field', h('span', 'Password'), pass), h('label.field', h('span', 'Again'), again), h('button.primary', { type: 'submit' }, 'Continue')),
    err);
}

/** Adds a passkey; answers the server's reply (recovery codes the first time). */
async function addPasskey(name) {
  const begin = await api('POST', '/me/passkeys/begin', {});
  const credential = await passkeyCreate(begin.options);
  return api('POST', '/me/passkeys/finish', { challenge_id: begin.challenge_id, credential, name });
}

function totpEnroller(onDone) {
  const err = h('p.err', { role: 'alert' });
  const box = h('div.stack', h('p.muted', 'Getting a code…'));
  (async () => {
    try {
      const t = await api('POST', '/me/totp/begin', {});
      // The server's QR code, shown as an image (an image can't run anything).
      const qr = h('div.qr', h('img', { src: 'data:image/svg+xml;base64,' + btoa(t.qr), alt: 'QR code for your authenticator app', width: 204, height: 204 }));
      const code = codeInput();
      box.replaceChildren(
        h('p.muted', 'Scan this with your authenticator app (1Password, Bitwarden, Google Authenticator, Authy…), then enter the code it shows.'),
        qr,
        h('details', h('summary.small.muted', 'Can\'t scan? Enter this key'), h('p.secret', t.secret.replace(/(.{4})/g, '$1 ').trim())),
        h('form.stack', {
          async onsubmit(ev) {
            ev.preventDefault();
            err.textContent = '';
            try { onDone(await api('POST', '/me/totp/confirm', { code: code.value })); } catch (e) { err.textContent = e.message; code.select(); }
          },
        }, h('label.field', h('span', 'Code'), code), h('button.primary', { type: 'submit' }, 'Confirm')),
        err);
      code.focus();
    } catch (e) { box.replaceChildren(h('p.err', e.message)); }
  })();
  return box;
}

function enrollView() {
  const err = h('p.err', { role: 'alert' });
  const finished = r => r.recovery_codes?.length ? recoveryCodesView(r.recovery_codes, () => boot()) : boot();
  const choice = h('div.stack',
    passkeysWork() && h('button.primary', {
      async onclick() {
        err.textContent = '';
        try { finished(await addPasskey(deviceName())); } catch (e) { err.textContent = passkeyError(e); }
      },
    }, keyIcon(), 'Add a passkey (recommended)'),
    h('button', { onclick() { choice.replaceChildren(totpEnroller(finished)); } }, 'Use an authenticator app instead'));
  gate(brand('Admin setup'), h('h1', 'Add a second factor'),
    h('p.muted', 'Every admin signs in with a password and a second factor. A passkey (Touch ID, Windows Hello, a security key or your phone) is the strongest, and signs you in on its own.'),
    choice, err);
}

function deviceName() {
  const ua = navigator.userAgent;
  const os = /iPhone|iPad/.test(ua) ? 'iPhone/iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'Device';
  return `${os} passkey`;
}

function recoveryCodesView(codes, then) {
  const ack = h('input', { type: 'checkbox' });
  const cont = h('button.primary', { disabled: true, onclick: then }, 'Continue');
  ack.addEventListener('change', () => { cont.disabled = !ack.checked; });
  const text = codes.join('\n');
  gate(brand(), h('h1', 'Save your recovery codes'),
    h('p.muted', 'If you lose your passkeys and authenticator, each of these signs you in once. Keep them somewhere safe, like a password manager. They won\'t be shown again.'),
    h('div.codes', codes.map(c => h('span', c))),
    h('div.row', h('button.small', {
      async onclick() {
        try { await navigator.clipboard.writeText(text); toast('Copied.'); } catch { toast('Couldn\'t copy: select them by hand.', true); }
      },
    }, 'Copy')),
    h('label.row', ack, h('span', 'I\'ve saved them')), cont);
}

/** Asks for a fresh second factor (for sensitive changes); true when given. */
function reverify() {
  const dialog = document.getElementById('modal');
  return new Promise(resolve => {
    const err = h('p.err', { role: 'alert' });
    const ses = state.session || {};
    const finish = ok => { dialog.close(); resolve(ok); };
    const code = codeInput();
    dialog.replaceChildren(h('div.stack',
      h('h2', 'Confirm it\'s you'),
      h('p.muted', 'This change needs your second factor again.'),
      ses.passkeys > 0 && passkeysWork() && h('button.primary', {
        async onclick() { try { await usePasskey(); finish(true); } catch (e) { err.textContent = passkeyError(e); } },
      }, keyIcon(), 'Use your passkey'),
      ses.totp && h('form.stack', {
        async onsubmit(ev) {
          ev.preventDefault();
          try { await call('POST', '/login/totp', { code: code.value }); finish(true); } catch (e) { err.textContent = e.message; }
        },
      }, h('label.field', h('span', 'Authenticator code'), code), h('button', { type: 'submit' }, 'Confirm')),
      err,
      h('div.row.end', h('button.ghost', { onclick: () => finish(false) }, 'Cancel'))));
    dialog.addEventListener('cancel', () => resolve(false), { once: true });
    dialog.showModal();
  });
}

/** A yes/no question in the page's own dialog. */
function confirmBox(title, body, yes = 'Confirm', danger = false) {
  const dialog = document.getElementById('modal');
  return new Promise(resolve => {
    const finish = v => { dialog.close(); resolve(v); };
    dialog.replaceChildren(h('div.stack', h('h2', title), h('p.muted', body),
      h('div.row.end', h('button.ghost', { onclick: () => finish(false) }, 'Cancel'), h(danger ? 'button.danger' : 'button.primary', { onclick: () => finish(true) }, yes))));
    dialog.addEventListener('cancel', () => resolve(false), { once: true });
    dialog.showModal();
  });
}

// ---------------------------------------------------------------- shell

function icon(d) {
  return s('svg', { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': 1.8, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' }, s('path', { d }));
}

function shell(content) {
  const cur = route().name;
  return h('div.shell',
    h('aside.side',
      brand(),
      h('nav.nav', ROUTES.map(([id, label, d]) => h('a', { href: `#/${id}`, 'aria-current': id === cur ? 'page' : null, title: label }, icon(d), h('span', label)))),
      h('div.who',
        h('span.small.muted', 'Signed in as'),
        h('b', state.session?.username || ''),
        h('button.ghost.small', { async onclick() { await call('POST', '/logout', {}); state.session = null; boot(); } }, 'Sign out'))),
    h('main.main', content));
}

async function render() {
  clearInterval(state.timer);
  const r = route();
  const views = { overview, servers, players, activity, network, updates, security, audit };
  const main = h('div.stack', h('div.empty', 'Loading…'));
  app.replaceChildren(shell(main));
  try {
    main.replaceChildren(...[await views[r.name](r.arg)].flat());
  } catch (e) {
    if (e.status !== 401) main.replaceChildren(h('div.panel', h('p.err', e.message)));
  }
  if (r.name === 'overview') state.timer = setInterval(() => { if (!document.hidden && route().name === 'overview') render(); }, 30000);
}

function top(title, sub, ...actions) {
  return h('div.top', h('div', h('h1', title), sub && h('p.sub', sub)), actions.length ? h('div.row', actions) : null);
}

function rangePicker(key, options, def) {
  const cur = state.range[key] ?? def;
  return h('div.seg', { role: 'group' }, options.map(([v, label]) =>
    h('button', { 'aria-pressed': String(v === cur), onclick() { state.range[key] = v; render(); } }, label)));
}
const rangeOf = (key, def) => state.range[key] ?? def;

// ---------------------------------------------------------------- charts

const PALETTE = ['#6cf09a', '#6cb8f0', '#f2b84b', '#c79bff', '#ff8f70', '#5fd7d0', '#e6e36a', '#f07cc4'];
const colorFor = (() => {
  const known = new Map();
  return id => { if (!known.has(id)) known.set(id, PALETTE[known.size % PALETTE.length]); return known.get(id); };
})();

/** A top for the y axis whose quarters are round numbers. */
function niceMax(v) {
  if (!(v > 0)) return 4;
  const quarter = v / 4;
  const p = 10 ** Math.floor(Math.log10(quarter));
  for (const m of [1, 2, 2.5, 5, 10]) if (quarter <= m * p) return 4 * m * p;
  return 40 * p;
}

/** A line chart. series: [{ name, color, points: [{ t, v }] }] over [from, to]. */
function lineChart(series, { from, to, format = v => fmt.n(v), height = 180, max } = {}) {
  const W = 720, H = height, L = 46, R = 8, T = 8, B = 22;
  const all = series.flatMap(se => se.points.filter(p => p.v != null).map(p => p.v));
  if (!all.length) return h('div.empty', 'No data for this period yet.');
  const top = max ?? niceMax(Math.max(...all) * 1.05);
  const x = t => L + (t - from) / Math.max(1, to - from) * (W - L - R);
  const y = v => T + (1 - Math.min(v, top) / top) * (H - T - B);
  const span = to - from;
  const tf = span > 3 * 86400 ? { month: 'short', day: 'numeric' } : { hour: '2-digit', minute: '2-digit' };
  const svg = s('svg', { viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': series.map(se => se.name).join(', ') });
  for (let i = 0; i <= 4; i++) {
    const v = top * i / 4;
    svg.append(s('line', { class: 'grid-line', x1: L, x2: W - R, y1: y(v), y2: y(v) }), s('text', { class: 'axis', x: L - 6, y: y(v) + 3.5, 'text-anchor': 'end' }, format(v)));
  }
  for (let i = 0; i <= 4; i++) {
    const t = from + span * i / 4;
    svg.append(s('text', { class: 'axis', x: x(t), y: H - 5, 'text-anchor': i === 0 ? 'start' : i === 4 ? 'end' : 'middle' }, new Date(t * 1000).toLocaleString(undefined, tf)));
  }
  const step = series.reduce((m, se) => {
    for (let i = 1; i < se.points.length; i++) m = Math.min(m, se.points[i].t - se.points[i - 1].t);
    return m;
  }, Infinity);
  for (const se of series) {
    let d = '', prev = null;
    for (const p of se.points) {
      if (p.v == null) { prev = null; continue; }
      // A gap of more than a few steps breaks the line (a server that was down).
      const jump = !prev || (Number.isFinite(step) && p.t - prev.t > step * 3.5);
      d += `${jump ? 'M' : 'L'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`;
      prev = p;
    }
    svg.append(s('path', { d, fill: 'none', style: `stroke:${se.color}`, 'stroke-width': 1.8, 'stroke-linejoin': 'round', 'vector-effect': 'non-scaling-stroke' }));
  }
  const hover = s('line', { class: 'hover-line', y1: T, y2: H - B, visibility: 'hidden' });
  svg.append(hover);
  const box = h('div.chart', svg);
  const tip = h('div.tip', { hidden: true });
  box.append(tip);
  const times = [...new Set(series.flatMap(se => se.points.map(p => p.t)))].sort((a, b) => a - b);
  svg.addEventListener('pointermove', ev => {
    const rect = svg.getBoundingClientRect();
    const px = (ev.clientX - rect.left) / rect.width * W;
    const t = from + (px - L) / (W - L - R) * span;
    let best = times[0];
    for (const tt of times) if (Math.abs(tt - t) < Math.abs(best - t)) best = tt;
    hover.setAttribute('x1', x(best)); hover.setAttribute('x2', x(best)); hover.setAttribute('visibility', 'visible');
    tip.replaceChildren(h('div.small.muted', new Date(best * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })),
      ...series.map(se => {
        const p = se.points.find(q => q.t === best);
        return h('div.k', h('span', h('i', { style: `background:${se.color}` }), ' ', se.name), h('b.num', p?.v == null ? '–' : format(p.v)));
      }));
    tip.hidden = false;
    const left = (x(best) / W) * rect.width;
    tip.style.left = `${Math.min(Math.max(left + 12, 0), rect.width - tip.offsetWidth)}px`;
    tip.style.top = '8px';
  });
  svg.addEventListener('pointerleave', () => { tip.hidden = true; hover.setAttribute('visibility', 'hidden'); });
  return box;
}

function legend(series) {
  return h('div.legend', series.map(se => h('span', h('i', { style: `background:${se.color}` }), se.name)));
}

function chartPanel(title, series, opts, extra) {
  return h('section.panel', h('header', h('h2', title), legend(series)), lineChart(series, opts), extra);
}

// ---------------------------------------------------------------- map

function worldMap(places, servers = [], unit = 'players') {
  const svg = s('svg', { viewBox: '0 6 360 152', role: 'img', 'aria-label': `Map of ${unit} by city` });
  for (let lon = -150; lon <= 150; lon += 30) svg.append(s('line', { class: 'graticule', x1: lon + 180, x2: lon + 180, y1: 0, y2: 180 }));
  for (let lat = -60; lat <= 60; lat += 30) svg.append(s('line', { class: 'graticule', x1: 0, x2: 360, y1: 90 - lat, y2: 90 - lat }));
  svg.append(s('path', { class: 'land', d: LAND }));
  const located = places.filter(p => p.country && (p.lat || p.lon));
  const top = Math.max(1, ...located.map(p => p.amount));
  for (const p of located.sort((a, b) => b.amount - a.amount)) {
    const r = 1.1 + Math.sqrt(p.amount / top) * 5.5;
    const cx = p.lon + 180, cy = 90 - p.lat;
    const label = `${[p.city, p.region, p.country_name || p.country].filter(Boolean).join(', ')}: ${fmt.n(p.amount)} ${unit}`;
    svg.append(s('circle', { class: 'spot', cx, cy, r }, s('title', {}, label)), s('circle', { class: 'spot core', cx, cy, r: 0.7 }));
  }
  for (const sv of servers.filter(sv => sv.place)) {
    const cx = sv.place.lon + 180, cy = 90 - sv.place.lat;
    svg.append(s('rect', { class: 'server', x: cx - 1.6, y: cy - 1.6, width: 3.2, height: 3.2, transform: `rotate(45 ${cx} ${cy})` }, s('title', {}, `${serverName(sv)} (server)`)));
  }
  return h('div.map', svg);
}

// ---------------------------------------------------------------- data helpers

const serverName = sv => sv.listing?.name || sv.id;

function serverStatus(sv) {
  if (!sv.online) return ['bad', 'offline'];
  if (sv.delisted) return ['warn', 'delisted'];
  return ['ok', 'online'];
}

function updatePill(sv, rollout) {
  const v = sv.listing?.version || '?';
  const u = sv.update?.updater || {};
  if (!sv.listing?.auto_update) return h('span.pill.warn', `${v} · updates off`);
  if (u.state === 'updating') return h('span.pill', `${v} → ${u.version}`);
  if (u.state === 'failed' || u.state === 'rolled-back') return h('span.pill.bad', `${v} · ${u.state} ${u.version || ''}`);
  if (rollout?.target && v !== rollout.target) return h('span.pill.warn', `${v} · behind`);
  return h('span.pill.ok', v);
}

async function loadOverview() {
  state.overview = await api('GET', '/overview');
  return state.overview;
}

function seriesFor(data, field, transform = v => v) {
  const ids = Object.keys(data.points).sort();
  const names = new Map((state.overview?.servers || []).map(sv => [sv.id, serverName(sv)]));
  return ids.map(id => ({ name: names.get(id) || id, color: colorFor(id), points: data.points[id].map(p => ({ t: p.t, v: transform(p[field], p) })) }));
}

function totalSeries(data, field, name = 'All servers') {
  const sums = new Map();
  for (const pts of Object.values(data.points)) for (const p of pts) sums.set(p.t, (sums.get(p.t) || 0) + (p[field] || 0));
  return { name, color: 'var(--text)', points: [...sums.entries()].sort((a, b) => a[0] - b[0]).map(([t, v]) => ({ t, v })) };
}

// ---------------------------------------------------------------- views

async function overview() {
  const [o, day, now] = await Promise.all([loadOverview(), api('GET', '/series?range=86400'), api('GET', '/places?range=0')]);
  const online = o.servers.filter(sv => sv.online);
  const players = online.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0);
  const inMatch = online.reduce((n, sv) => n + (sv.metrics?.players?.in_match || 0), 0);
  const sessions = online.reduce((n, sv) => n + (sv.metrics?.activity || []).reduce((m, a) => m + (a.sessions || 0), 0), 0);
  const r = o.rollout;
  const stage = r.target ? `${r.target} · ${r.paused ? 'paused' : r.stage}` : 'no release yet';
  const playerLines = seriesFor(day, 'players');
  if (playerLines.length > 1) playerLines.unshift(totalSeries(day, 'players'));
  return [
    top('Overview', `Coordinator ${o.coordinator.version} · ${online.length} of ${o.servers.length} servers online · updated ${new Date().toLocaleTimeString()}`),
    h('div.kpis',
      h('div.kpi', h('span.label', 'Players online'), h('span.v', fmt.n(players)), h('span.d', `${fmt.n(inMatch)} in a match`)),
      h('div.kpi', h('span.label', 'Peak, 24 h'), h('span.v', fmt.n(Math.max(o.peak_24h || 0, players))), h('span.d', 'most online at once')),
      h('div.kpi', h('span.label', 'Sessions'), h('span.v', fmt.n(sessions)), h('span.d', 'lobbies and matches')),
      h('div.kpi', h('span.label', 'Servers'), h('span.v', fmt.n(online.length), h('small', `/ ${o.servers.length}`)), h('span.d', o.servers.some(sv => sv.delisted) ? `${o.servers.filter(sv => sv.delisted).length} delisted` : 'all listed')),
      h('div.kpi', h('span.label', 'Release'), h('span.v', { style: 'font-size:18px' }, r.target || '–'), h('span.d', stage))),
    h('div.grid.cols-2',
      chartPanel('Players online, 24 hours', playerLines, { from: day.from, to: day.to }),
      h('section.panel', h('header', h('h2', 'Where players are now'), h('a.small', { href: '#/players' }, 'Players & map →')),
        worldMap(now.places, o.servers, 'players'),
        h('p.attr', now.attribution))),
    h('section.panel', h('header', h('h2', 'Servers'), h('a.small', { href: '#/servers' }, 'Details →')), serverTable(o)),
  ];
}

function serverTable(o) {
  return h('div.table-wrap', h('table',
    h('thead', h('tr', h('th', 'Server'), h('th', 'Region'), h('th', 'Version'), h('th.r', 'Players'), h('th.r', 'CPU'), h('th.r', 'Memory'), h('th.r', 'Ping'), h('th', 'Seen'))),
    h('tbody', o.servers.map(sv => {
      const m = sv.metrics, sys = m?.system || {};
      const [cls, label] = serverStatus(sv);
      return h('tr',
        h('td', h('a', { href: `#/servers/${encodeURIComponent(sv.id)}` }, h('span.row', h(`span.dot.${cls}`, { title: label }), serverName(sv)))),
        h('td.muted', sv.listing?.region || '–'),
        h('td', updatePill(sv, o.rollout)),
        h('td.r', fmt.n(m?.players?.online)),
        h('td.r', fmt.pct(sys.cpu_percent)),
        h('td.r', sys.mem_total ? fmt.pct((sys.mem_total - sys.mem_available) / sys.mem_total * 100) : '–'),
        h('td.r', fmt.ms(sv.ping_ms)),
        h('td.muted', fmt.ago(sv.last_seen)));
    }))));
}

function bar(label, frac, text) {
  const f = Math.max(0, Math.min(1, frac || 0));
  return h('div.bar', h('span.muted', label), h('div.track', h(`div.fill${f > 0.9 ? '.bad' : f > 0.75 ? '.warn' : ''}`, { style: `width:${(f * 100).toFixed(1)}%` })), h('span.num', { style: 'text-align:right' }, text));
}

async function servers(id) {
  if (id) return serverDetail(decodeURIComponent(id));
  const [o, hour] = await Promise.all([loadOverview(), api('GET', '/series?range=3600')]);
  const last = sid => (hour.points[sid] || []).at(-1) || {};
  return [
    top('Servers', 'Every member of the network, as each last reported.'),
    o.servers.length ? h('div.servers', o.servers.map(sv => {
      const m = sv.metrics || {}, sys = m.system || {}, l = last(sv.id);
      const [cls, label] = serverStatus(sv);
      return h('section.panel.server',
        h('header', h('div', h('h2', h(`span.dot.${cls}`, { title: label }), h('a', { href: `#/servers/${encodeURIComponent(sv.id)}` }, serverName(sv))),
          h('p.meta', [sv.listing?.region, sv.listing?.host].filter(Boolean).join(' · '))), updatePill(sv, o.rollout)),
        sv.delisted && h('p.callout.warn', `Delisted: ${sv.delisted}`),
        !sv.online && h('p.callout.bad', `Offline since ${fmt.when(sv.last_seen)}`),
        (sv.name_clashes || []).map(clash => h('p.callout.warn', `Name: ${clash}`)),
        h('div.bars',
          bar('CPU', (sys.cpu_percent || 0) / 100, fmt.pct(sys.cpu_percent)),
          bar('Memory', sys.mem_total ? (sys.mem_total - sys.mem_available) / sys.mem_total : 0, sys.mem_total ? `${fmt.bytes(sys.mem_total - sys.mem_available)}` : '–'),
          bar('Disk', sys.disk_total ? 1 - sys.disk_free / sys.disk_total : 0, sys.disk_total ? `${fmt.bytes(sys.disk_free)} free` : '–')),
        h('div.facts',
          h('div.fact', h('b', fmt.n(m.players?.online)), h('span', 'players online')),
          h('div.fact', h('b', fmt.rate(l.rx)), h('span', 'in')),
          h('div.fact', h('b', fmt.rate(l.tx)), h('span', 'out')),
          h('div.fact', h('b', fmt.ms(sv.ping_ms)), h('span', 'ping from coordinator')),
          h('div.fact', h('b', fmt.dur(m.uptime_secs)), h('span', 'server uptime')),
          h('div.fact', h('b', sys.load ? sys.load[0].toFixed(2) : '–'), h('span', `load · ${sys.cpus || '?'} CPUs`))));
    })) : h('div.panel.empty', 'No servers have joined yet. Install one with install-server.sh and the join token.'),
  ];
}

const RANGES = [[3600, '1 h'], [21600, '6 h'], [86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '1 y']];

async function serverDetail(id) {
  const range = rangeOf('server', 86400);
  const [o, data] = await Promise.all([loadOverview(), api('GET', `/series?range=${range}`)]);
  const sv = o.servers.find(x => x.id === id);
  if (!sv) return [top('Server not found'), h('p', h('a', { href: '#/servers' }, '← All servers'))];
  const pts = data.points[id] || [];
  const opts = { from: data.from, to: data.to };
  const one = (name, field, color, f) => ({ name, color, points: pts.map(p => ({ t: p.t, v: f ? f(p) : p[field] })) });
  const pingPts = (data.pings[id] || []).map(p => ({ t: p.t, v: p.ms }));
  const sys = sv.metrics?.system || {};
  return [
    top(serverName(sv), [sv.listing?.region, sv.listing?.host, `id ${sv.id}`].filter(Boolean).join(' · '),
      rangePicker('server', RANGES, 86400),
      h('button.small', {
        async onclick() {
          if (!await confirmBox(`Release ${serverName(sv)}'s unused names?`, 'Its links made over an hour ago for identities never seen online, and linked nowhere else, go with the names only they held. For a server reserving names with throwaway identities.', 'Release names', true)) return;
          try { toast((await api('POST', `/servers/${encodeURIComponent(id)}/purge-names`)).message); } catch (e) { toast(e.message, true); }
        },
      }, 'Release unused names'),
      h('button.danger.small', {
        async onclick() {
          if (!await confirmBox(`Remove ${serverName(sv)}?`, 'Its links and the names only it used go. It can join again with the join token unless you make a new one.', 'Remove server', true)) return;
          try { toast((await api('DELETE', `/servers/${encodeURIComponent(id)}`)).message); location.hash = '#/servers'; } catch (e) { toast(e.message, true); }
        },
      }, 'Remove')),
    h('p', h('a.small', { href: '#/servers' }, '← All servers')),
    h('div.grid.cols-2',
      chartPanel('Players', [one('Online', 'players', PALETTE[0]), one('In a match', 'in_match', PALETTE[1])], opts),
      chartPanel('CPU', [one('Machine', 'cpu', PALETTE[0]), one('Server process', 'process_cpu', PALETTE[2])], { ...opts, format: v => fmt.pct(v), max: 100 }),
      chartPanel('Memory', [one('Used', 'mem_used', PALETTE[0]), one('Server process', 'rss', PALETTE[2])], { ...opts, format: fmt.bytes, max: sys.mem_total || undefined }),
      chartPanel('Bandwidth', [one('In', 'rx', PALETTE[1]), one('Out', 'tx', PALETTE[0]), one('Relayed', 'relayed', PALETTE[2])], { ...opts, format: fmt.rate }),
      chartPanel('Sign-ins per minute', [one('Game', 'logins', PALETTE[0]), one('Failed', 'failed_logins', PALETTE[4]), one('New accounts', 'registrations', PALETTE[1])], { ...opts, format: v => fmt.n(v, 1) }),
      chartPanel('Ping from the coordinator', [{ name: 'Round trip', color: PALETTE[3], points: pingPts }], { ...opts, format: fmt.ms })),
  ];
}

async function players() {
  const range = rangeOf('players', 0);
  const [o, data] = await Promise.all([loadOverview(), api('GET', `/places?range=${range}`)]);
  const unit = data.unit;
  const names = new Map(o.servers.map(sv => [sv.id, serverName(sv)]));
  const total = data.places.reduce((n, p) => n + p.amount, 0);
  return [
    top('Players & map', range === 0 ? 'Players online now, by city.' : 'Time played, by city (player-minutes).',
      rangePicker('players', [[0, 'Now'], [86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '1 y']], 0)),
    h('section.panel', worldMap(data.places, o.servers, unit), h('div.row', { style: 'justify-content:space-between;margin-top:8px' },
      h('span.legend', h('span', h('i', { style: 'background:var(--accent)' }), unit === 'players' ? 'players' : 'player-minutes'), h('span', h('i', { style: 'background:var(--warn)' }), 'servers')),
      h('span.attr', data.attribution))),
    h('section.panel', h('header', h('h2', 'By city'), h('span.muted.small', `${fmt.n(total)} ${unit} · ${data.places.length} places`)),
      data.places.length ? h('div.table-wrap', h('table',
        h('thead', h('tr', h('th', 'City'), h('th', 'Region'), h('th', 'Country'), h('th.r', unit === 'players' ? 'Players' : 'Minutes'), h('th.r', 'Share'), h('th', 'Servers'))),
        h('tbody', data.places.map(p => h('tr',
          h('td', p.city || (p.country ? '–' : 'Unknown or private network')),
          h('td.muted', p.region || '–'),
          h('td', p.country ? `${flag(p.country)} ${p.country_name || p.country}` : '–'),
          h('td.r', fmt.n(p.amount)),
          h('td.r', fmt.pct(total ? p.amount / total * 100 : 0)),
          h('td.muted.small', Object.entries(p.servers || {}).sort((a, b) => b[1] - a[1]).map(([sid, n]) => `${names.get(sid) || sid} ${fmt.n(n)}`).join(' · '))))))) : h('div.empty', range === 0 ? 'Nobody is playing right now.' : 'No players in this period.')),
  ];
}

function flag(cc) {
  return /^[A-Z]{2}$/.test(cc) ? String.fromCodePoint(...[...cc].map(c => 0x1f1a5 + c.charCodeAt(0))) : '';
}

async function activity() {
  const range = rangeOf('activity', 0);
  const data = await api('GET', `/activity?range=${range}`);
  const label = (kind, id) => {
    if (id == null) return h('span.faint', '–');
    const l = data.labels.find(x => x.kind === kind && x.id === id);
    return h('span.row', l ? h('span', l.name) : h('span.muted', `#${id}`),
      h('button.ghost.small', { title: 'Name it', onclick: () => nameIt(kind, id, l?.name) }, l ? 'Rename' : 'Name'));
  };
  const total = data.activity.reduce((n, a) => n + a.players, 0);
  const now = range === 0;
  return [
    top('Playlists', now ? 'What\'s being played right now, by mode and map.' : 'What was played, by mode and map (player-minutes).',
      rangePicker('activity', [[0, 'Now'], [86400, '24 h'], [604800, '7 d'], [2592000, '30 d'], [31536000, '1 y']], 0)),
    h('section.panel',
      data.activity.length ? h('div.table-wrap', h('table',
        h('thead', h('tr', h('th', 'Mode'), h('th', 'Room'), h('th', 'Map'), h('th', 'Game mode'), h('th.r', now ? 'Players' : 'Player-min'), h('th.r', now ? 'Sessions' : 'Session-min'), h('th', 'Share'))),
        h('tbody', data.activity.map(a => h('tr',
          h('td', a.mode === 'svm' ? 'Spies vs Mercs' : 'Co-op'),
          h('td.muted', a.room === 'match' ? 'Match' : 'Lobby'),
          h('td', label('map', a.map)),
          h('td', label('game_mode', a.game_mode)),
          h('td.r', fmt.n(a.players)),
          h('td.r', fmt.n(a.sessions)),
          h('td', { style: 'min-width:120px' }, bar('', total ? a.players / total : 0, fmt.pct(total ? a.players / total * 100 : 0)))))))) : h('div.empty', now ? 'No lobbies or matches right now.' : 'Nothing was played in this period.'),
      h('p.small.muted', { style: 'margin-top:10px' }, 'The game reports maps and modes by number. Name them as you identify them; the names apply everywhere.')),
  ];
}

async function nameIt(kind, id, current) {
  const dialog = document.getElementById('modal');
  const input = h('input', { value: current || '', maxlength: 48, placeholder: kind === 'map' ? 'e.g. Penthouse' : 'e.g. Blacklist' });
  dialog.replaceChildren(h('form.stack', {
    async onsubmit(ev) {
      ev.preventDefault();
      try { await api('PUT', '/labels', { kind, id, name: input.value }); dialog.close(); render(); } catch (e) { toast(e.message, true); }
    },
  }, h('h2', `Name ${kind === 'map' ? 'map' : 'game mode'} #${id}`), h('label.field', h('span', 'Name (empty removes it)'), input),
  h('div.row.end', h('button.ghost', { type: 'button', onclick: () => dialog.close() }, 'Cancel'), h('button.primary', { type: 'submit' }, 'Save'))));
  dialog.showModal();
  input.focus();
}

async function network() {
  const range = rangeOf('network', 86400);
  const [o, data, pings] = await Promise.all([loadOverview(), api('GET', `/series?range=${range}`), api('GET', `/pings?range=${Math.max(range, 86400)}`)]);
  const opts = { from: data.from, to: data.to };
  const names = new Map(o.servers.map(sv => [sv.id, serverName(sv)]));
  const pingLines = Object.keys(data.pings).sort().map(id => ({ name: names.get(id) || id, color: colorFor(id), points: data.pings[id].map(p => ({ t: p.t, v: p.ms })) }));
  const byServer = new Map();
  for (const p of pings.pings) { if (!byServer.has(p.server)) byServer.set(p.server, []); byServer.get(p.server).push(p); }
  return [
    top('Network', 'Bandwidth, relayed traffic and round trips.', rangePicker('network', RANGES.slice(0, 5), 86400)),
    h('div.grid.cols-2',
      chartPanel('Traffic out', seriesFor(data, 'tx'), { ...opts, format: fmt.rate }),
      chartPanel('Traffic in', seriesFor(data, 'rx'), { ...opts, format: fmt.rate }),
      chartPanel('Relayed between players', seriesFor(data, 'relayed'), { ...opts, format: fmt.rate }),
      chartPanel('Ping from the coordinator', pingLines, { ...opts, format: fmt.ms })),
    h('section.panel', h('header', h('h2', 'Players\' ping to each server'), h('span.small.muted', 'As their launchers measured it, by country · last ' + (range > 86400 ? RANGES.find(r => r[0] === range)?.[1] : '24 h'))),
      byServer.size ? h('div.grid.cols-2', [...byServer.entries()].map(([sid, rows]) => h('div',
        h('h3', { style: 'margin-bottom:6px' }, names.get(sid) || sid),
        h('table', h('thead', h('tr', h('th', 'Country'), h('th.r', 'Median'), h('th.r', '90th %'), h('th.r', 'Reports'))),
          h('tbody', rows.sort((a, b) => b.reports - a.reports).map(p => h('tr',
            h('td', p.country ? `${flag(p.country)} ${p.country}` : 'Unknown'),
            h('td.r', fmt.ms(p.median)), h('td.r', fmt.ms(p.p90)), h('td.r', fmt.n(p.reports))))))))) : h('div.empty', 'No launcher has reported pings yet.')),
  ];
}

async function updates() {
  const [o, u] = await Promise.all([loadOverview(), api('GET', '/updates')]);
  const r = u.rollout;
  const order = ['canary', 'verifying', 'rolling', 'done'];
  const at = order.indexOf(r.stage);
  const act = (action, label, cls = '', ask, body) => h(`button${cls}`, {
    async onclick() {
      if (ask && !await confirmBox(ask[0], ask[1], label, cls.includes('danger'))) return;
      try { toast((await api('POST', `/updates/${action}`, body || {})).message); render(); } catch (e) { toast(e.message, true); }
    },
  }, label);
  const stageText = {
    canary: r.canary ? `${names(o, r.canary)} installs it first` : 'choosing a server to try it on',
    verifying: `watching ${names(o, r.canary)} on the new release`,
    rolling: 'the rest update, each when it\'s quiet (or within 2 hours)',
    done: 'every server runs it',
  };
  return [
    top('Updates', `Releases from github.com/${u.repo}, signed with the release key. Servers check the signature again before installing.`,
      act('check', 'Check GitHub now')),
    h('section.panel',
      h('header', h('div', h('span.label', 'Rollout'), h('h2', { style: 'font-size:18px;margin-top:2px' }, r.target ? `Release ${r.target}` : 'Nothing rolled out yet')),
        h('div.row', r.paused && h('span.pill.warn', 'paused'), r.pinned && h('span.pill', 'pinned'), r.stage === 'halted' && h('span.pill.bad', 'halted'))),
      r.target && h('div.stages', order.map((st, i) => h(`div.stage${r.stage === 'halted' && i === Math.max(at, 0) ? '.halted' : i < at ? '.done' : i === at ? '.now' : ''}`,
        h('b', st[0].toUpperCase() + st.slice(1)), i === at ? stageText[st] : ''))),
      r.note && h(`p.callout${r.stage === 'halted' ? '.bad' : ''}`, r.note),
      r.target && h('p.small.muted', { style: 'margin-top:8px' }, `Since ${fmt.when(r.stage_since)} · previous release ${r.previous || 'none recorded'}`),
      h('div.row', { style: 'margin-top:14px' },
        r.paused ? act('resume', 'Resume') : act('pause', 'Pause'),
        r.pinned ? act('unpin', 'Unpin (follow new releases)') : act('pin', 'Pin (hold this release)'),
        ['canary', 'verifying', 'halted'].includes(r.stage) && act('promote', 'Skip the canary', '', ['Update every server now?', 'Every server installs the release without waiting for the canary.']),
        r.stage !== 'halted' && r.stage !== 'done' && r.target && act('halt', 'Halt'),
        r.previous && act('rollback', `Roll back to ${r.previous}`, '.danger', [`Roll back to ${r.previous}?`, 'Every server goes back to the release before (each kept it), and the rollout is pinned there.']))),
    h('section.panel', h('header', h('h2', 'Servers')),
      h('div.table-wrap', h('table',
        h('thead', h('tr', h('th', 'Server'), h('th', 'Runs'), h('th', 'Updates'), h('th', 'Updater'), h('th', 'Directory'))),
        h('tbody', o.servers.map(sv => {
          const up = sv.update?.updater || {};
          return h('tr',
            h('td', serverName(sv)),
            h('td', updatePill(sv, r)),
            h('td', sv.listing?.auto_update ? h('span.pill.ok', 'on') : h('span.pill.bad', 'off')),
            h('td.small.muted', up.state ? `${up.state}${up.version ? ' ' + up.version : ''}${up.at ? ' · ' + fmt.ago(up.at) : ''}${up.error ? ' · ' + up.error : ''}` : '–'),
            h('td.small', sv.delisted ? h('span.pill.warn', { title: sv.delisted }, 'delisted') : h('span.pill.ok', 'listed')));
        }))))),
    h('section.panel', h('header', h('h2', 'Signed releases')),
      u.releases.length ? h('div.table-wrap', h('table',
        h('thead', h('tr', h('th', 'Release'), h('th', 'Published'), h('th', ''))),
        h('tbody', u.releases.map(rel => h('tr',
          h('td', h('a', { href: rel.page, target: '_blank', rel: 'noopener noreferrer' }, rel.version), rel.version === r.target && h('span.pill.ok', { style: 'margin-left:8px' }, 'target')),
          h('td.muted', rel.published_at ? new Date(rel.published_at).toLocaleString() : fmt.when(rel.seen_at)),
          h('td.r', rel.version !== r.target && act('release', 'Roll this out', '.small', [`Roll out ${rel.version}?`, 'A canary first, then the rest. The rollout is pinned to it.'], { version: rel.version }))))))) : h('div.empty', 'No signed release seen yet. Releases appear here once published and signed (scripts/sign-release.sh).')),
  ];
}

const names = (o, id) => serverName(o.servers.find(sv => sv.id === id) || { id });

async function security() {
  const [me, admins, rules] = await Promise.all([api('GET', '/me'), api('GET', '/admins'), api('GET', '/restrictions')]);
  const ses = await call('GET', '/session');
  state.session = { ...state.session, ...ses };
  return [
    top('Security', 'Your sign-in, the other admins, and where the admin UI may be reached from.'),
    h('div.grid.cols-2',
      accountPanel(me),
      sessionsPanel(me)),
    restrictionsPanel(rules),
    adminsPanel(admins, me),
  ];
}

function accountPanel(me) {
  const add = async () => {
    try {
      const r = await addPasskey(deviceName());
      if (r.recovery_codes?.length) toast('Passkey added. New recovery codes were made.');
      else toast('Passkey added.');
      render();
    } catch (e) { toast(passkeyError(e), true); }
  };
  return h('section.panel', h('header', h('h2', 'Your account'), h('span.muted', me.username)),
    h('div.stack',
      h('div', h('span.label', 'Passkeys'),
        me.passkeys.length ? h('table', h('tbody', me.passkeys.map(p => h('tr',
          h('td', p.name), h('td.small.muted', `added ${fmt.when(p.created_at)} · used ${fmt.ago(p.last_used)}`),
          h('td.r', h('button.ghost.small', {
            async onclick() {
              if (!await confirmBox(`Remove "${p.name}"?`, 'It won\'t sign you in any more.', 'Remove', true)) return;
              try { await api('DELETE', `/me/passkeys/${encodeURIComponent(p.id)}`); render(); } catch (e) { toast(e.message, true); }
            },
          }, 'Remove')))))) : h('p.muted.small', 'None yet.'),
        passkeysWork() && h('button.small', { style: 'margin-top:8px', onclick: add }, keyIcon(), 'Add a passkey')),
      h('div', h('span.label', 'Authenticator app'), h('div.row', { style: 'margin-top:6px' },
        me.totp ? h('span.pill.ok', 'on') : h('span.pill', 'off'),
        me.totp ? h('button.ghost.small', {
          async onclick() {
            if (!await confirmBox('Turn off the authenticator app?', 'Your passkeys still sign you in.', 'Turn off', true)) return;
            try { await api('DELETE', '/me/totp'); render(); } catch (e) { toast(e.message, true); }
          },
        }, 'Turn off') : h('button.small', {
          onclick() {
            const dialog = document.getElementById('modal');
            dialog.replaceChildren(h('div.stack', h('h2', 'Add an authenticator app'), totpEnroller(() => { dialog.close(); toast('Authenticator app added.'); render(); }),
              h('div.row.end', h('button.ghost', { onclick: () => dialog.close() }, 'Cancel'))));
            dialog.showModal();
          },
        }, 'Set up'))),
      h('div', h('span.label', 'Recovery codes'), h('div.row', { style: 'margin-top:6px' },
        h('span.small', `${me.recovery_left} of 10 left`),
        h('button.ghost.small', {
          async onclick() {
            if (!await confirmBox('Make new recovery codes?', 'The old ones stop working.', 'Make new codes')) return;
            try {
              const r = await api('POST', '/me/recovery', {});
              const dialog = document.getElementById('modal');
              dialog.replaceChildren(h('div.stack', h('h2', 'Your new recovery codes'), h('p.muted', 'Save them now: they won\'t be shown again.'), h('div.codes', r.recovery_codes.map(c => h('span', c))),
                h('div.row.end', h('button.primary', { onclick: () => { dialog.close(); render(); } }, 'I\'ve saved them'))));
              dialog.showModal();
            } catch (e) { toast(e.message, true); }
          },
        }, 'Make new codes'))),
      passwordForm()));
}

function passwordForm() {
  const cur = h('input', { type: 'password', autocomplete: 'current-password' });
  const nw = h('input', { type: 'password', autocomplete: 'new-password', minlength: 12 });
  return h('details', h('summary.small', 'Change password'), h('form.stack', {
    style: 'margin-top:10px',
    async onsubmit(ev) {
      ev.preventDefault();
      try { await api('POST', '/me/password', { current: cur.value, new: nw.value }); toast('Password changed. Your other sessions ended.'); render(); } catch (e) { toast(e.message, true); }
    },
  }, h('label.field', h('span', 'Current password'), cur), h('label.field', h('span', 'New password'), nw), h('div.row.end', h('button', { type: 'submit' }, 'Change password'))));
}

function sessionsPanel(me) {
  return h('section.panel', h('header', h('h2', 'Your sessions')),
    h('div.table-wrap', h('table', h('tbody', me.sessions.map(se => h('tr',
      h('td', h('div', se.current ? h('b', 'This browser') : browserName(se.user_agent)), h('div.small.muted', `${se.ip}${se.country ? ' · ' + flag(se.country) + ' ' + se.country : ''}`)),
      h('td.small.muted', `active ${fmt.ago(se.last_seen)}`),
      h('td.r', !se.current && h('button.ghost.small', {
        async onclick() { try { await api('DELETE', `/sessions/${se.id}`); render(); } catch (e) { toast(e.message, true); } },
      }, 'End'))))))));
}

function browserName(ua) {
  const b = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  const os = /iPhone|iPad/.test(ua) ? 'iOS' : /Android/.test(ua) ? 'Android' : /Mac OS/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  return `${b}${os ? ' on ' + os : ''}`;
}

function restrictionsPanel(rules) {
  const nets = h('textarea', { placeholder: 'One per line, e.g.\n203.0.113.7\n198.51.100.0/24\n2001:db8::/48' }, rules.networks.join('\n'));
  const countries = h('input', { value: rules.countries.join(', '), placeholder: 'e.g. NZ, AU' });
  return h('section.panel', h('header', h('h2', 'Where admins may sign in from'), h('span.small.muted', `You: ${rules.you.ip}${rules.you.country ? ' · ' + flag(rules.you.country) + ' ' + rules.you.country : ''}`)),
    h('form.grid.cols-2', {
      async onsubmit(ev) {
        ev.preventDefault();
        try {
          await api('PUT', '/restrictions', { networks: nets.value.split(/[\n,]+/).map(x => x.trim()).filter(Boolean), countries: countries.value.split(/[\s,]+/).filter(Boolean) });
          toast('Saved. Requests from anywhere else are refused, the sign-in page too.');
          render();
        } catch (e) { toast(e.message, true); }
      },
    },
    h('label.field', h('span', 'Networks (empty: any)'), nets),
    h('div.stack', h('label.field', h('span', 'Countries (empty: any)'), countries),
      h('p.small.muted', 'Countries come from Cloudflare. Rules that would lock you out are refused. Locked out anyway? On the coordinator\'s machine: coordinator admin open-access.'),
      h('div.row.end', h('button.primary', { type: 'submit' }, 'Save rules')))));
}

function adminsPanel(list, me) {
  const name = h('input', { placeholder: 'new admin\'s name', maxlength: 32, style: 'max-width:240px' });
  const showLink = (who, link) => {
    const dialog = document.getElementById('modal');
    dialog.replaceChildren(h('div.stack', h('h2', `Setup link for ${who}`),
      h('p.muted', 'Send it privately. It works once, within 24 hours, to choose a password and add a second factor.'),
      h('p.secret', link),
      h('div.row.end', h('button', { async onclick() { try { await navigator.clipboard.writeText(link); toast('Copied.'); } catch { toast('Select it by hand.', true); } } }, 'Copy'),
        h('button.primary', { onclick: () => { dialog.close(); render(); } }, 'Done'))));
    dialog.showModal();
  };
  const action = (a, act, label, cls = '.ghost.small', ask) => h(`button${cls}`, {
    async onclick() {
      if (ask && !await confirmBox(ask[0], ask[1], label, true)) return;
      try { const r = await api('POST', `/admins/${a.id}/${act}`, {}); r.link ? showLink(a.username, r.link) : render(); } catch (e) { toast(e.message, true); }
    },
  }, label);
  return h('section.panel', h('header', h('h2', 'Admins'),
    h('form.row', {
      async onsubmit(ev) {
        ev.preventDefault();
        try { const r = await api('POST', '/admins', { username: name.value.trim() }); showLink(name.value.trim(), r.link); } catch (e) { toast(e.message, true); }
      },
    }, name, h('button.small', { type: 'submit' }, 'Add admin'))),
  h('div.table-wrap', h('table',
    h('thead', h('tr', h('th', 'Admin'), h('th', 'Second factors'), h('th', 'Last sign-in'), h('th', ''))),
    h('tbody', list.admins.map(a => h('tr',
      h('td', h('b', a.username), a.username === me.username && h('span.faint', ' (you)'), a.disabled && h('span.pill.bad', { style: 'margin-left:8px' }, 'disabled'), !a.set_up && h('span.pill.warn', { style: 'margin-left:8px' }, 'setup pending')),
      h('td.small', [a.passkeys ? `${a.passkeys} passkey${a.passkeys > 1 ? 's' : ''}` : null, a.totp ? 'authenticator' : null].filter(Boolean).join(' · ') || h('span.faint', 'none')),
      h('td.small.muted', fmt.ago(a.last_login)),
      h('td.r', a.username !== me.username && h('div.row.end',
        action(a, 'reset', 'Reset sign-in', '.ghost.small', [`Reset ${a.username}'s sign-in?`, 'Their password, passkeys and authenticator are cleared and they\'re signed out. You get a new setup link for them.']),
        a.disabled ? action(a, 'enable', 'Enable') : action(a, 'disable', 'Disable', '.ghost.small', [`Disable ${a.username}?`, 'They\'re signed out and can\'t sign in until enabled.']),
        action(a, 'remove', 'Remove', '.danger.small', [`Remove ${a.username}?`, 'Their account is deleted.'])))))))));
}

async function audit() {
  const data = await api('GET', '/audit');
  const body = h('tbody');
  const rows = events => events.map(e => h('tr',
    h('td.small.muted', { style: 'white-space:nowrap' }, fmt.when(e.at)),
    h('td', e.admin || h('span.faint', '–')),
    h('td', e.event),
    h('td.small.muted', e.detail),
    h('td.small.muted', `${e.ip}${e.country ? ' · ' + e.country : ''}`)));
  add(body, rows(data.events));
  let last = data.events.at(-1)?.id;
  const more = h('button.small', {
    hidden: data.events.length < 200,
    async onclick() {
      const next = await api('GET', `/audit?before=${last}`);
      add(body, rows(next.events));
      last = next.events.at(-1)?.id;
      more.hidden = next.events.length < 200;
    },
  }, 'Older');
  return [
    top('Audit log', 'Sign-ins, failures and every change, newest first. Kept for 400 days.'),
    h('section.panel', data.events.length ? h('div.table-wrap', h('table', h('thead', h('tr', h('th', 'When'), h('th', 'Admin'), h('th', 'Event'), h('th', 'Detail'), h('th', 'From'))), body)) : h('div.empty', 'Nothing yet.'),
      h('div.row', { style: 'margin-top:10px' }, more)),
  ];
}

boot();
