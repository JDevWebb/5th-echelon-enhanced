import { createRouter, createWebHashHistory } from 'vue-router';

// The rail: [id, label, icon (SVG shapes, 24 × 24, stroked)]; null draws a separator.
export const ROUTES = [
  ['overview', 'Overview', '<rect x="3" y="3" width="7" height="8" rx="1.5"/><rect x="14" y="3" width="7" height="5" rx="1.5"/><rect x="14" y="12" width="7" height="9" rx="1.5"/><rect x="3" y="15" width="7" height="6" rx="1.5"/>'],
  ['servers', 'Servers', '<rect x="3" y="4" width="18" height="6" rx="2"/><rect x="3" y="14" width="18" height="6" rx="2"/><path d="M7 7h.01M7 17h.01"/>'],
  ['players', 'Players', '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18.5 14.8c1.6.8 2.6 2.5 3 5.2"/>'],
  ['live', 'Live', '<circle cx="12" cy="12" r="2.5"/><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M4.9 4.9a10 10 0 0 0 0 14.2M19.1 4.9a10 10 0 0 1 0 14.2"/>'],
  ['sessions', 'Sessions', '<circle cx="12" cy="12" r="8.5"/><path d="M12 7v5l3.5 2"/>'],
  ['reports', 'Reports', '<path d="M5 4h10l4 4v12H5z"/><path d="M15 4v4h4M8.5 12.5h7M8.5 16h5"/>'],
  ['alerts', 'Alerts', '<path d="M12 3.5 21.5 20h-19z"/><path d="M12 10v4.5M12 17.5h.01"/>'],
  ['updates', 'Updates', '<path d="M20 12a8 8 0 1 1-2.3-5.6"/><path d="M20 4v5h-5"/>'],
  ['support', 'Support', '<path d="M4 5h16v11H9l-4 4v-4H4z"/><path d="M8 9h8M8 12.5h5"/>'],
  ['roadmap', 'Roadmap', '<path d="M4 19V5M4 5h11l-2 3.5L15 12H4"/><path d="M15 19h5M18 16l2 3-2 3"/>'],
  null,
  ['network', 'Network', '<circle cx="12" cy="5" r="2"/><circle cx="5" cy="19" r="2"/><circle cx="19" cy="19" r="2"/><path d="M12 7v4M12 11l-6 6M12 11l6 6"/>'],
  ['security', 'Security', '<path d="M12 3 19 6v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z"/><path d="m9 12 2 2 4-4"/>'],
];

/** Pages shown as tabs of another page in the rail: Bandwidth and Activity under Network,
 * the audit log under Security. */
export const PARENT = { bandwidth: 'network', activity: 'network', audit: 'security', match: 'sessions' };
export const NETWORK_TABS = [['/network', 'Traffic'], ['/bandwidth', 'Bandwidth'], ['/activity', 'Activity']];
export const SECURITY_TABS = [['/security', 'Sign-in and admins'], ['/audit', 'Audit log']];

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/overview' },
    { path: '/overview', component: () => import('./views/Overview.vue') },
    { path: '/servers', component: () => import('./views/Servers.vue') },
    { path: '/servers/:id', component: () => import('./views/ServerDetail.vue'), props: true },
    { path: '/bandwidth', component: () => import('./views/Bandwidth.vue') },
    { path: '/live', component: () => import('./views/Live.vue') },
    { path: '/match/:server/:room/:since', component: () => import('./views/Match.vue'), props: true },
    { path: '/sessions', component: () => import('./views/Sessions.vue') },
    { path: '/players', component: () => import('./views/Players.vue') },
    { path: '/players/:server/:id', component: () => import('./views/Players.vue') },
    { path: '/alerts', component: () => import('./views/Alerts.vue') },
    { path: '/reports', component: () => import('./views/Reports.vue') },
    { path: '/reports/:id', component: () => import('./views/Reports.vue') },
    { path: '/activity', component: () => import('./views/Playlists.vue') },
    { path: '/network', component: () => import('./views/Network.vue') },
    { path: '/updates', component: () => import('./views/Updates.vue') },
    { path: '/roadmap', component: () => import('./views/Roadmap.vue') },
    { path: '/support', component: () => import('./views/Support.vue') },
    { path: '/support/:identity', component: () => import('./views/Support.vue') },
    { path: '/security', component: () => import('./views/Security.vue') },
    { path: '/audit', component: () => import('./views/Audit.vue') },
    { path: '/:rest(.*)*', redirect: '/overview' },
  ],
});
