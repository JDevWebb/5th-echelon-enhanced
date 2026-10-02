// Passkeys (WebAuthn) for signing in and for adding one.
import { call, api } from './api.js';

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

export const passkeysWork = () => !!(window.PublicKeyCredential && navigator.credentials);

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
export async function usePasskey() {
  const begin = await call('POST', '/login/passkey/begin', {});
  const credential = await passkeyGet(begin.options);
  return call('POST', '/login/passkey/finish', { challenge_id: begin.challenge_id, credential });
}

/** Adds a passkey; answers the server's reply (recovery codes the first time). */
export async function addPasskey(name = deviceName()) {
  const begin = await api('POST', '/me/passkeys/begin', {});
  const credential = await passkeyCreate(begin.options);
  return api('POST', '/me/passkeys/finish', { challenge_id: begin.challenge_id, credential, name });
}

export function passkeyError(e) {
  if (e?.name === 'NotAllowedError') return 'The passkey prompt was cancelled or timed out.';
  if (e?.name === 'InvalidStateError') return 'That passkey is registered already.';
  return e?.message || String(e);
}

export function deviceName() {
  const ua = navigator.userAgent;
  const os = /iPhone|iPad/.test(ua) ? 'iPhone/iPad' : /Android/.test(ua) ? 'Android' : /Mac/.test(ua) ? 'Mac' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'Device';
  return `${os} passkey`;
}
