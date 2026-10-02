// Who's signed in, and how far: the sign-in stages ('password', 'enroll', 'full').
import { reactive } from 'vue';
import { call } from './api.js';

export const session = reactive({
  /** 'loading', 'login', 'password' (second factor next), 'setup', 'enroll', 'codes', 'full' */
  view: 'loading',
  info: null,
  you: null,
  error: '',
  setupToken: null,
  codes: [],

  async boot() {
    const setup = location.hash.match(/^#setup=([A-Za-z0-9]+)$/);
    if (setup) {
      // Out of the address bar and history at once.
      history.replaceState(null, '', location.pathname);
      this.setupToken = setup[1];
      this.view = 'setup';
      return;
    }
    try {
      this.info = await call('GET', '/session');
    } catch (e) {
      this.info = null;
      this.you = e.body?.you || null;
      this.view = 'login';
      return;
    }
    this.view = this.info.stage === 'full' ? 'full' : this.info.stage;
  },

  signedOut() {
    if (this.view === 'full') {
      this.info = null;
      this.boot();
    }
  },

  showCodes(codes) {
    this.codes = codes;
    this.view = 'codes';
  },

  async signOut() {
    try { await call('POST', '/logout', {}); } catch { /* signed out either way */ }
    this.info = null;
    this.boot();
  },
});
