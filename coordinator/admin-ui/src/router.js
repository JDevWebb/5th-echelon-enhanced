import { createRouter, createWebHashHistory } from 'vue-router';

export const ROUTES = [
  ['overview', 'Overview', 'M3 13h4v8H3zM10 3h4v18h-4zM17 9h4v12h-4z'],
  ['servers', 'Servers', 'M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01'],
  ['bandwidth', 'Bandwidth', 'M7 4v16M7 20l-3-3M7 20l3-3M17 20V4M17 4l-3 3M17 4l3 3'],
  ['sessions', 'Sessions', 'M3 6h8M3 12h13M3 18h6M14 4v4M19 10v4M11 16v4'],
  ['players', 'Players & map', 'M12 21s-7-6.2-7-11a7 7 0 1 1 14 0c0 4.8-7 11-7 11zM12 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z'],
  ['activity', 'Playlists', 'M4 6h16M4 12h10M4 18h13'],
  ['network', 'Network', 'M2 12h4l3-8 4 16 3-8h6'],
  ['alerts', 'Alerts', 'M12 3a6 6 0 0 0-6 6v4l-2 3h16l-2-3V9a6 6 0 0 0-6-6zM10 19a2 2 0 0 0 4 0'],
  ['reports', 'Reports', 'M5 4h14v12H9l-4 4zM9 8h6M9 12h4'],
  ['updates', 'Updates', 'M12 3v12m0 0-4-4m4 4 4-4M5 21h14'],
  ['security', 'Security', 'M12 3 4 6v6c0 5 3.4 8.2 8 9 4.6-.8 8-4 8-9V6z'],
  ['audit', 'Audit log', 'M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01'],
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/overview' },
    { path: '/overview', component: () => import('./views/Overview.vue') },
    { path: '/servers', component: () => import('./views/Servers.vue') },
    { path: '/servers/:id', component: () => import('./views/ServerDetail.vue'), props: true },
    { path: '/bandwidth', component: () => import('./views/Bandwidth.vue') },
    { path: '/sessions', component: () => import('./views/Sessions.vue') },
    { path: '/players', component: () => import('./views/Players.vue') },
    { path: '/players/:server/:id', component: () => import('./views/Players.vue') },
    { path: '/alerts', component: () => import('./views/Alerts.vue') },
    { path: '/reports', component: () => import('./views/Reports.vue') },
    { path: '/reports/:id', component: () => import('./views/Reports.vue') },
    { path: '/activity', component: () => import('./views/Playlists.vue') },
    { path: '/network', component: () => import('./views/Network.vue') },
    { path: '/updates', component: () => import('./views/Updates.vue') },
    { path: '/security', component: () => import('./views/Security.vue') },
    { path: '/audit', component: () => import('./views/Audit.vue') },
    { path: '/:rest(.*)*', redirect: '/overview' },
  ],
});
