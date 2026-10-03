// Player actions (ban, kick, …) wait for the player's server to carry them out: it hears
// about them with its next pulse (within ten seconds) and answers. This follows them.
import { reactive } from 'vue';
import { api } from './api.js';

/** Actions being followed: { id: { kind, status, message } }. */
export const following = reactive({});

const sleep = ms => new Promise(r => setTimeout(r, ms));

/** Asks for action `id` until it's done, failed or expired (every 2 s, then every 10 s
 * after a minute). Resolves with its last state: a reset password comes in it once. */
export async function follow(id, kind) {
  following[id] = { kind, status: 'pending', message: '' };
  const started = Date.now();
  try {
    for (;;) {
      await sleep(Date.now() - started < 60000 ? 2000 : 10000);
      let a;
      try { a = await api('GET', `/actions/${id}`); } catch (e) { if (e.status === 404 || e.status === 401) throw e; continue; }
      following[id] = { kind, status: a.status, message: a.message || '' };
      if (a.status !== 'pending') return a;
    }
  } finally {
    setTimeout(() => { delete following[id]; }, 8000);
  }
}

export const ACTION_LABELS = {
  ban: 'Ban', unban: 'Unban', kick: 'Kick', reset_password: 'Reset password', rename: 'Rename', delete: 'Delete',
};
