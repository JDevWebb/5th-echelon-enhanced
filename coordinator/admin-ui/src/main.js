// First: a setup link leaves the address bar before the router (made as it loads) rewrites it.
import { onSetupLink } from './lib/setupLink.js';
import { createApp } from 'vue';
import App from './App.vue';
import { router } from './router.js';
import { session } from './lib/session.js';
// Before anything shows: the colours the admin chose (or the system's).
import './lib/theme.js';
import './styles.css';

// A setup link opened in a tab already showing the admin UI (only its hash changes).
onSetupLink(() => session.boot());

session.boot().finally(() => createApp(App).use(router).mount('#app'));
