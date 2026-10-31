// A map's geometry for the Replay page's 3D view: the files tools/map_geometry
// (map-geometry) writes from the admin's own copy of the game. Read here as it writes them
// (gzip around "FEMG", version 1, the name, the meshes, the instances); kept in this browser
// (IndexedDB) once loaded, never sent anywhere.

/** `{ name, meshes: [{ positions: Float32Array, indices: Uint32Array }], instances:
 * [{ mesh, kind, placement: Float32Array(12) }] }` from a .femg file's bytes. */
export async function readMap(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
  const raw = new Uint8Array(await new Response(stream).arrayBuffer());
  const v = new DataView(raw.buffer);
  let at = 0;
  const u32 = () => { const x = v.getUint32(at, true); at += 4; return x; };
  if (String.fromCharCode(...raw.subarray(0, 4)) !== 'FEMG') throw new Error('not a map geometry file (make one with map-geometry)');
  at = 4;
  const version = u32();
  if (version !== 1) throw new Error(`a map file of version ${version}; this page reads 1`);
  const n = raw[at];
  const name = new TextDecoder().decode(raw.subarray(at + 1, at + 1 + n));
  at += 1 + n;
  const meshes = [];
  for (let i = 0, count = u32(); i < count; i++) {
    const nv = u32();
    const nt = u32();
    // Copied out: the views must start on 4-byte boundaries.
    const positions = new Float32Array(raw.slice(at, at + nv * 12).buffer);
    at += nv * 12;
    const indices = new Uint32Array(raw.slice(at, at + nt * 12).buffer);
    at += nt * 12;
    meshes.push({ positions, indices });
  }
  const instances = [];
  for (let i = 0, count = u32(); i < count; i++) {
    const mesh = u32();
    const kind = raw[at];
    at += 1;
    const placement = new Float32Array(raw.slice(at, at + 48).buffer);
    at += 48;
    if (mesh < meshes.length) instances.push({ mesh, kind, placement });
  }
  return { name, meshes, instances };
}

// The browser's own store of map files, by map name (case ignored). Any failure (a private
// window, storage turned off) only means the file is asked for again.
const DB = 'fes-replay-maps';
function open() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore('maps');
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

export async function keepMap(name, bytes) {
  try {
    const db = await open();
    await new Promise((resolve, reject) => {
      const tx = db.transaction('maps', 'readwrite');
      tx.objectStore('maps').put(bytes, name.toLowerCase());
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
    });
    db.close();
  } catch { /* kept for this visit only */ }
}

export async function keptMap(name) {
  if (!name) return null;
  try {
    const db = await open();
    const bytes = await new Promise((resolve, reject) => {
      const req = db.transaction('maps').objectStore('maps').get(name.toLowerCase());
      req.onsuccess = () => resolve(req.result || null);
      req.onerror = () => reject(req.error);
    });
    db.close();
    return bytes;
  } catch {
    return null;
  }
}
