// THIRD-PARTY-NOTICES.txt for a release (build.sh notices; the release workflow): every library the
// released files carry and its licence, with the licences' full texts.
//
//   node build/notices.js OUT ADMIN_UI_DIR FONTS_DIR CARGO_ABOUT_JSON...
//
// The Rust crates come from cargo-about's JSON (about.toml), one file per released program, merged:
// each crate once, each licence text once, with the crates under it. This project's own crates
// (path dependencies outside vendor/) are left out. The admin UI's production npm packages (what
// the coordinator embeds) come from its package-lock.json and node_modules. Problems (a crate or
// package without a licence text) go to stderr and fail the run.
'use strict';
const fs = require('fs');
const path = require('path');

const [out, adminUi, fontsDir, ...jsons] = process.argv.slice(2);
if (!out || !adminUi || !fontsDir || jsons.length === 0) {
  console.error('usage: node build/notices.js OUT ADMIN_UI_DIR FONTS_DIR CARGO_ABOUT_JSON...');
  process.exit(2);
}
const problems = [];
const rule = '='.repeat(78);
const thin = '-'.repeat(78);
const norm = (t) => t.replace(/\r\n?/g, '\n').replace(/[ \t]+$/gm, '').trim();
const wrap = (items, width = 78) => {
  const lines = [];
  let line = '';
  for (const item of items) {
    const next = line ? `${line}, ${item}` : item;
    if (next.length > width && line) {
      lines.push(`${line},`);
      line = item;
    } else line = next;
  }
  if (line) lines.push(line);
  return lines.join('\n');
};

// id -> { name, texts: Map(text -> Set("name version")), users: Set("name version") }
function collect() {
  const byId = new Map();
  const add = (id, name, text, user) => {
    if (!byId.has(id)) byId.set(id, { name, texts: new Map(), users: new Set() });
    const l = byId.get(id);
    const t = norm(text);
    if (!l.texts.has(t)) l.texts.set(t, new Set());
    l.texts.get(t).add(user);
    l.users.add(user);
  };
  return { byId, add };
}
const byName = (a, b) => a.localeCompare(b, 'en', { numeric: true });

// --- Rust crates -------------------------------------------------------------------------------
const rust = collect();
const crates = new Map(); // "name version" -> { expr, dir }
const firstParty = (pkg) => !pkg.source && !pkg.manifest_path.includes('/vendor/');
for (const file of jsons) {
  const about = JSON.parse(fs.readFileSync(file, 'utf8'));
  for (const c of about.crates) {
    if (firstParty(c.package)) continue;
    crates.set(`${c.package.name} ${c.package.version}`, { expr: c.license, dir: path.dirname(c.package.manifest_path) });
  }
  for (const l of about.licenses) {
    for (const u of l.used_by) {
      if (firstParty(u.crate)) continue;
      rust.add(l.id, l.name, l.text, `${u.crate.name} ${u.crate.version}`);
    }
  }
}
// A LicenseRef (Slint's royalty-free licence) cargo-about accepted but has no text for: the
// crate's own LICENSES/<id>.md or .txt (the REUSE layout).
const covered = new Set([...rust.byId.values()].flatMap((l) => [...l.users]));
for (const [c, { expr, dir }] of crates) {
  if (covered.has(c)) continue;
  const ref = (expr.match(/LicenseRef-[\w.-]+/g) || [])
    .map((id) => [id, ['.md', '.txt'].map((ext) => path.join(dir, 'LICENSES', id + ext)).find((f) => fs.existsSync(f))])
    .find(([, f]) => f);
  if (ref) rust.add(ref[0], ref[0], fs.readFileSync(ref[1], 'utf8'), c);
  else problems.push(`crate ${c} (${expr}): no licence text`);
}

// --- The admin UI's npm packages -----------------------------------------------------------------
const npm = collect();
const lock = JSON.parse(fs.readFileSync(path.join(adminUi, 'package-lock.json'), 'utf8'));
let npmCount = 0;
for (const [key, entry] of Object.entries(lock.packages)) {
  if (!key.startsWith('node_modules/') || entry.dev || entry.devOptional) continue;
  const dir = path.join(adminUi, key);
  if (!fs.existsSync(dir)) {
    if (!entry.optional) problems.push(`npm ${key}: not installed (npm ci first)`);
    continue;
  }
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
  const licence = typeof pkg.license === 'string' ? pkg.license : (pkg.license && pkg.license.type) || 'UNKNOWN';
  const user = `${pkg.name} ${pkg.version}`;
  const files = fs.readdirSync(dir).filter((f) => /^(licen[cs]e|copying)/i.test(f)).sort();
  npmCount++;
  if (files.length === 0) {
    // Named, but without its text (@vue/devtools-api): said so, under the same licence's text
    // from the other packages.
    const author = typeof pkg.author === 'string' ? pkg.author : pkg.author && pkg.author.name;
    npm.add(licence, licence, `${pkg.name} names ${licence} as its licence${author ? ` (author: ${author})` : ''} but carries no licence file.`, user);
    console.error(`npm ${user} (${licence}): no licence file, listed by its package.json`);
    continue;
  }
  for (const f of files) npm.add(licence, licence, fs.readFileSync(path.join(dir, f), 'utf8'), user);
}

// --- Writing ----------------------------------------------------------------------------------
const section = (title, { byId }) => {
  const ids = [...byId.keys()].sort((a, b) => byId.get(b).users.size - byId.get(a).users.size || byName(a, b));
  const parts = [];
  for (const id of ids) {
    const l = byId.get(id);
    parts.push(`${rule}\n${title}: ${id}${l.name && l.name !== id ? ` (${l.name})` : ''}\n${l.users.size} ${l.users.size === 1 ? 'library' : 'libraries'}\n${rule}\n\n${wrap([...l.users].sort(byName))}\n`);
    for (const [text, users] of l.texts) {
      parts.push(`${thin}\n${id}, as given by: ${wrap([...users].sort(byName))}\n${thin}\n\n${text}\n`);
    }
  }
  return parts.join('\n');
};
const summary = ({ byId }) =>
  [...byId.entries()]
    .sort((a, b) => b[1].users.size - a[1].users.size || byName(a[0], b[0]))
    .map(([id, l]) => `  ${String(l.users.size).padStart(4)}  ${id}`)
    .join('\n');

const fonts = fs
  .readdirSync(fontsDir)
  .filter((f) => /^OFL-.*\.txt$/.test(f))
  .sort()
  .map((f) => `${thin}\n${f.replace(/^OFL-|\.txt$/g, '')}\n${thin}\n\n${norm(fs.readFileSync(path.join(fontsDir, f), 'utf8'))}\n`);

const text = `5th Echelon Enhanced: third-party notices
${rule}

The release files (launcher.exe, launcher-linux-x86_64, uplay_r1_loader.dll, dedicated_server,
coordinator, testbot and map-geometry) are built with the open source libraries below, which are
used under their own licences. This file lists each licence once, the libraries under it (name and
version), and the licences' full texts as the libraries give them. A library offered under a
choice of licences is listed under the one used; one under several licences at once is listed
under each, so the counts add up to more than the total. Every library is used unchanged but
hudhook, whose patched copy is in this project's repository (vendor/hudhook). Their sources are
on crates.io (Rust; imgui and imgui-sys from github.com/imgui-rs/imgui-rs at cb8beeb) and
npmjs.com (the coordinator's admin UI) at the versions listed. This project's own licence is in
LICENSE.md in its repository.

Rust libraries: ${crates.size}
${summary(rust)}

The coordinator's admin UI (web page, embedded in the coordinator), npm packages: ${npmCount}
${summary(npm)}

Fonts: IBM Plex Sans, Sans Condensed and Mono (SIL Open Font License 1.1), in the launcher, the
client DLL's overlay and the admin UI.

Data: the admin UI's world map is Natural Earth's 1:110m land outline (public domain), via
world-atlas (ISC licence). SQLite, which the servers and the coordinator build in (through
libsqlite3-sys), is in the public domain.

${section('Rust libraries', rust)}
${section('Admin UI (npm)', npm)}
${rule}
Fonts: IBM Plex
${rule}

${fonts.join('\n')}`;

fs.writeFileSync(out, text);
if (problems.length) {
  console.error(problems.join('\n'));
  console.error(`${problems.length} problem(s): fix them (about.toml clarifications) before releasing`);
  process.exit(1);
}
console.error(`${out}: ${crates.size} crates, ${npmCount} npm packages, ${(text.length / 1024).toFixed(0)} KiB`);
