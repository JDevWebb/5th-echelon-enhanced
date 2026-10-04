// The admin API. Every change carries X-FES-Admin (the coordinator refuses
// changes without it, so other sites can't make them with the admin's cookie).
import { session } from './session.js';
import { openModal } from './ui.js';

export class ApiError extends Error {
  constructor(status, body) { super(body.error || `HTTP ${status}`); this.status = status; this.body = body; }
}

export async function call(method, path, body) {
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

/** A call from a signed-in page: asks to confirm it's them when the server wants a fresh
 * second factor, and goes back to signing in when the session ended. */
export async function api(method, path, body) {
  try {
    return await call(method, path, body);
  } catch (e) {
    if (e.status === 403 && e.body?.reverify) {
      const { default: Reverify } = await import('../components/Reverify.vue');
      if (await openModal(Reverify)) return call(method, path, body);
      throw new Error('Not confirmed.');
    }
    if (e.status === 401) session.signedOut();
    throw e;
  }
}

/** A text file from the API (a report's file), its first `limit` bytes: `{ text, cut }`. */
export async function fetchText(path, limit) {
  const resp = await fetch('/api' + path, { credentials: 'same-origin' });
  if (!resp.ok) {
    const body = await resp.json().catch(() => ({}));
    if (resp.status === 401) session.signedOut();
    throw new ApiError(resp.status, body);
  }
  const reader = resp.body.getReader();
  const chunks = [];
  let size = 0;
  let cut = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    size += value.length;
    if (size > limit) { cut = true; reader.cancel().catch(() => {}); break; }
  }
  const bytes = new Uint8Array(Math.min(size, limit));
  let at = 0;
  for (const c of chunks) {
    const take = Math.min(c.length, bytes.length - at);
    bytes.set(c.subarray(0, take), at);
    at += take;
    if (at >= bytes.length) break;
  }
  return { text: new TextDecoder().decode(bytes), cut };
}
