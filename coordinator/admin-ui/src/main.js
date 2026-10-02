import { createApp } from 'vue';
import App from './App.vue';
import { router } from './router.js';
import { session } from './lib/session.js';
import './styles.css';

// A setup link (#setup=…) opened in a tab already showing the admin UI only changes the
// hash. Listening before the router does, this sees it before the router redirects it away.
window.addEventListener('hashchange', () => { if (/^#setup=/.test(location.hash)) session.boot(); });

// The session first: a setup link leaves the address bar before the router reads it.
session.boot().finally(() => createApp(App).use(router).mount('#app'));
