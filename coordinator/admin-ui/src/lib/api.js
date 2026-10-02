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
