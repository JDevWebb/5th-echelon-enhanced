// Who's signed in, and how far: the sign-in stages ('password', 'enroll', 'full').
import { reactive } from 'vue';
import { call } from './api.js';
import { setupToken } from './setupLink.js';

export const session = reactive({
  /** 'loading', 'login', 'password' (second factor next), 'setup', 'enroll', 'codes', 'full' */
  view: 'loading',
  info: null,
  you: null,
  error: '',
  setupToken: null,
  codes: [],

  async boot() {
    const token = setupToken();
    if (token) {
      this.setupToken = token;
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
