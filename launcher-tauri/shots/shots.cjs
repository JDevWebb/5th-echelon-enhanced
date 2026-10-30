// Screenshots of every screen of the Tauri launcher at several window sizes, and checks on each,
// for looking at it without Windows (build.sh tauri-shots -> dist/tauri-shots/*.png). The page
// built into launcher-tauri/ui/dist runs in a headless Chromium as the browser preview, on the
// made-up answers in ui/src/lib/tauri.js; its URL flags (?answered, ?notice…) set the scene.
//
// Each shot also fails on what a player would see go wrong: an error on the page, the page
// wider than the window, or a button with no name for a screen reader.
//
//   node shots.cjs <dist folder> <out folder>
const http = require('http');
const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

const [dist, out] = process.argv.slice(2);
if (!dist || !out) {
  console.error('usage: node shots.cjs <dist folder> <out folder>');
  process.exit(2);
}

// The window sizes looked at, as the egui shots: the first window's, a small laptop's, full HD.
const SIZES = [['1280x800', 1280, 800], ['960x600', 960, 600], ['1920x1080', 1920, 1080]];

const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.ttf': 'font/ttf', '.svg': 'image/svg+xml', '.png': 'image/png', '.jpg': 'image/jpeg', '.txt': 'text/plain' };

/** dist/ on a port of its own on this machine only. */
function serve() {
  const root = path.resolve(dist);
  const server = http.createServer((req, res) => {
    const file = path.join(root, decodeURIComponent(new URL(req.url, 'http://x').pathname));
    if (!file.startsWith(root)) return res.writeHead(403).end();
    const at = fs.existsSync(file) && fs.statSync(file).isDirectory() ? path.join(file, 'index.html') : file;
    fs.readFile(at, (err, body) => {
      if (err) return res.writeHead(404).end();
      res.writeHead(200, { 'content-type': TYPES[path.extname(at)] ?? 'application/octet-stream' }).end(body);
    });
  });
  return new Promise((ok) => server.listen(0, '127.0.0.1', () => ok(server)));
}

// Ways to the screens, as a player gets there.
const rail = (label) => async (page) => page.locator(`nav.rail button[aria-label="${label}"]`).click();
const button = (name) => async (page) => page.getByRole('button', { name }).first().click();
const tab = (name) => async (page) => page.getByRole('tab', { name }).first().click();
const settings = (label) => [rail('Settings'), async (page) => page.locator('nav.nav').getByRole('button', { name: label, exact: true }).click()];

const SHOTS = [
  ['play', '?answered', []],
  ['play-server-menu', '?answered', [async (page) => page.locator('button.server').click()]],
  ['play-mode-menu', '?answered', [async (page) => page.locator('button.play-more').click()]],
  ['play-squad', '?answered', [tab(/^Squad/)]],
  ['play-intel', '?answered', [tab('Intel')]],
  ['play-notice', '?answered&notice', []],
  ['play-banners', '?answered&banners&update', []],
  ['play-reduced-motion', '?answered', [], { reducedMotion: 'reduce' }],
  ['first-diagnostics', '', []],
  ['first-game-options', '', [button('Keep sending')]],
  ['setup-server', '?first&answered', []],
  ['setup-name', '?first&answered', [async (page) => page.getByRole('radio').first().click(), button('Connect'), async (page) => page.getByRole('button', { name: 'Create account' }).waitFor()]],
  ['servers', '?answered', [rail('Servers')]],
  ['host', '?answered', [rail('Servers'), button('Host a server')]],
  ['chat', '?answered', [rail('Chat')]],
  ['intel-briefings', '?answered', [rail('Intel: news and the roadmap')]],
  ['intel-roadmap', '?answered', [rail('Intel: news and the roadmap'), tab('Roadmap')]],
  ['support', '?answered', [rail('Support')]],
  ['whats-new', '?answered&whatsnew', []],
  ['feedback', '?answered&feedback', []],
  ...[
    ['game', 'Game'], ['network', 'Network'], ['servers', 'Servers and accounts'], ['identity', 'Identity and friends'],
    ['notifications', 'Notifications'], ['save', 'Save game'], ['client', '5th Echelon client'], ['advanced', 'Advanced'],
    ['display', 'Display'], ['feedback', 'Feedback'], ['about', 'About'],
  ].map(([id, label]) => [`settings-${id}`, '?answered&beta', settings(label)]),
];

/** What's wrong on the page as it is now: wider than the window, buttons without a name. */
function problemsOnPage() {
  const found = [];
  const doc = document.scrollingElement;
  if (doc.scrollWidth > window.innerWidth + 1) found.push(`the page is ${doc.scrollWidth}px wide in a ${window.innerWidth}px window`);
  for (const el of document.querySelectorAll('.page, .screen')) {
    if (el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).overflowX !== 'hidden') {
      found.push(`${el.className.split(' ')[0]} scrolls sideways (${el.scrollWidth}px in ${el.clientWidth}px)`);
    }
  }
  for (const b of document.querySelectorAll('button, [role="button"], [role="tab"], [role="radio"]')) {
    const shown = b.getClientRects().length > 0 && getComputedStyle(b).visibility !== 'hidden';
    const name = (b.getAttribute('aria-label') || b.textContent || b.getAttribute('title') || '').trim();
    if (shown && !name) found.push(`a button without a name: ${b.outerHTML.slice(0, 120)}`);
    // Cut off at the side by the window or by a panel that hides what overflows it.
    if (!shown) continue;
    const r = b.getBoundingClientRect();
    let left = 0;
    let right = window.innerWidth;
    for (let a = b.parentElement; a; a = a.parentElement) {
      if (getComputedStyle(a).overflowX === 'visible') continue;
      const ar = a.getBoundingClientRect();
      left = Math.max(left, ar.left);
      right = Math.min(right, ar.right);
    }
    if (r.width > 0 && (r.right > right + 1 || r.left < left - 1)) found.push(`cut off at the side: "${name.slice(0, 40)}"`);
  }
  return found;
}

(async () => {
  const server = await serve();
  const base = `http://127.0.0.1:${server.address().port}/index.html`;
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const failed = [];
  // A shot at its sizes side by side, each in a window of its own.
  for (const [name, query, steps, options = {}] of SHOTS) {
    await Promise.all(SIZES.map(async ([size, width, height]) => {
      const context = await browser.newContext({ viewport: { width, height }, reducedMotion: options.reducedMotion ?? 'no-preference' });
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', (e) => errors.push(`page error: ${e.message}`));
      page.on('console', (m) => m.type() === 'error' && errors.push(`console: ${m.text()}`));
      try {
        await page.goto(base + query);
        await page.locator('nav.rail').waitFor();
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(400);
        for (const step of steps) {
          await step(page);
          await page.waitForTimeout(400);
        }
        // Long enough for every screen's opening to settle (Play's is the longest, 1 s).
        await page.waitForTimeout(1200);
        const file = path.join(out, `${name}-${size}.png`);
        await page.screenshot({ path: file });
        console.log(file);
        errors.push(...(await page.evaluate(problemsOnPage)));
      } catch (e) {
        errors.push(String(e.message ?? e).split('\n')[0]);
      }
      for (const e of new Set(errors)) failed.push(`${name} at ${size}: ${e}`);
      await context.close();
    }));
  }
  await browser.close();
  server.close();
  if (failed.length) {
    console.error(`\n${failed.length} problem(s):\n  ${failed.join('\n  ')}`);
    process.exit(1);
  }
  console.log(`\nevery screen at ${SIZES.length} sizes, no problems`);
})();
