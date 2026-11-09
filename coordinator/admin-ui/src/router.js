import { createRouter, createWebHashHistory } from 'vue-router';

// The side menu, in groups: [id, label, icon (SVG shapes, 24 × 24, stroked)]. A group with
// no title comes first. Support work starts in the Inbox, which gathers what the other pages
// hold; each page still deals with its own.
const ICON = {
  inbox: '<path d="M3 13h5l2 3h4l2-3h5"/><path d="M5 5h14l2 8v6H3v-6z"/>',
  players: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18.5 14.8c1.6.8 2.6 2.5 3 5.2"/>',
  support: '<path d="M4 5h16v11H9l-4 4v-4H4z"/><path d="M8 9h8M8 12.5h5"/>',
  reports: '<path d="M5 4h10l4 4v12H5z"/><path d="M15 4v4h4M8.5 12.5h7M8.5 16h5"/>',
  moderation: '<path d="M4 5h16v11H9l-4 4v-4H4z"/><path d="M12 8v3.5M12 13.5h.01"/>',
  overview: '<rect x="3" y="3" width="7" height="8" rx="1.5"/><rect x="14" y="3" width="7" height="5" rx="1.5"/><rect x="14" y="12" width="7" height="9" rx="1.5"/><rect x="3" y="15" width="7" height="6" rx="1.5"/>',
  servers: '<rect x="3" y="4" width="18" height="6" rx="2"/><rect x="3" y="14" width="18" height="6" rx="2"/><path d="M7 7h.01M7 17h.01"/>',
  live: '<circle cx="12" cy="12" r="2.5"/><path d="M7.8 7.8a6 6 0 0 0 0 8.4M16.2 7.8a6 6 0 0 1 0 8.4M4.9 4.9a10 10 0 0 0 0 14.2M19.1 4.9a10 10 0 0 1 0 14.2"/>',
  sessions: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7v5l3.5 2"/>',
  network: '<circle cx="12" cy="5" r="2"/><circle cx="5" cy="19" r="2"/><circle cx="19" cy="19" r="2"/><path d="M12 7v4M12 11l-6 6M12 11l6 6"/>',
  crashes: '<path d="M12 3 3.5 18h17z"/><path d="M12 10v4M12 16.5h.01"/>',
  updates: '<path d="M20 12a8 8 0 1 1-2.3-5.6"/><path d="M20 4v5h-5"/>',
  roadmap: '<path d="M4 19V5M4 5h11l-2 3.5L15 12H4"/><path d="M15 19h5M18 16l2 3-2 3"/>',
  alerts: '<path d="M12 3.5 21.5 20h-19z"/><path d="M12 10v4.5M12 17.5h.01"/>',
  security: '<path d="M12 3 19 6v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z"/><path d="m9 12 2 2 4-4"/>',
};
const item = (id, label) => [id, label, ICON[id]];
export const NAV = [
  ['', [item('inbox', 'Inbox'), item('players', 'Players')]],
  ['Support', [item('support', 'Support'), item('reports', 'Reports'), item('moderation', 'Chat and names')]],
  ['Network', [item('overview', 'Overview'), item('servers', 'Servers'), item('live', 'Live matches'), item('sessions', 'Sessions'), item('network', 'Traffic')]],
  ['Game health', [item('crashes', 'Crashes')]],
  ['Operations', [item('updates', 'Releases'), item('roadmap', 'Roadmap'), item('alerts', 'Alerts'), item('security', 'Admins and security')]],
];

/** Pages shown as tabs of another page in the rail: Bandwidth and Activity under Network,
 * the audit log under Security. */
export const PARENT = { bandwidth: 'network', activity: 'network', audit: 'security', match: 'sessions' };
export const NETWORK_TABS = [['/network', 'Traffic'], ['/bandwidth', 'Bandwidth'], ['/activity', 'Activity']];
export const SECURITY_TABS = [['/security', 'Sign-in and admins'], ['/audit', 'Audit log']];

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/inbox' },
    { path: '/inbox', component: () => import('./views/Inbox.vue') },
    { path: '/overview', component: () => import('./views/Overview.vue') },
    { path: '/servers', component: () => import('./views/Servers.vue') },
    { path: '/servers/:id', component: () => import('./views/ServerDetail.vue'), props: true },
    { path: '/bandwidth', component: () => import('./views/Bandwidth.vue') },
    { path: '/live', component: () => import('./views/Live.vue') },
    { path: '/match/:server/:room/:since', component: () => import('./views/Match.vue'), props: true },
    { path: '/sessions', component: () => import('./views/Sessions.vue') },
    { path: '/replay/:server/:player/:id', component: () => import('./views/Replay.vue'), props: true },
    { path: '/players', component: () => import('./views/Players.vue') },
    { path: '/players/:server/:id', component: () => import('./views/Players.vue') },
    { path: '/alerts', component: () => import('./views/Alerts.vue') },
    { path: '/crashes', component: () => import('./views/Crashes.vue') },
    { path: '/reports', component: () => import('./views/Reports.vue') },
    { path: '/reports/:id', component: () => import('./views/Reports.vue') },
    { path: '/activity', component: () => import('./views/Playlists.vue') },
    { path: '/network', component: () => import('./views/Network.vue') },
    { path: '/updates', component: () => import('./views/Updates.vue') },
    { path: '/roadmap', component: () => import('./views/Roadmap.vue') },
    { path: '/support', component: () => import('./views/Support.vue') },
    { path: '/support/:identity', component: () => import('./views/Support.vue') },
    { path: '/moderation', component: () => import('./views/Moderation.vue') },
    { path: '/security', component: () => import('./views/Security.vue') },
    { path: '/audit', component: () => import('./views/Audit.vue') },
    { path: '/:rest(.*)*', redirect: '/inbox' },
  ],
});
