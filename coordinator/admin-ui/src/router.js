import { createRouter, createWebHashHistory } from 'vue-router';

export const ROUTES = [
  ['overview', 'Overview', 'M3 13h4v8H3zM10 3h4v18h-4zM17 9h4v12h-4z'],
  ['servers', 'Servers', 'M4 4h16v6H4zM4 14h16v6H4zM7 7h.01M7 17h.01'],
  ['players', 'Players & map', 'M12 21s-7-6.2-7-11a7 7 0 1 1 14 0c0 4.8-7 11-7 11zM12 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z'],
  ['activity', 'Playlists', 'M4 6h16M4 12h10M4 18h13'],
  ['network', 'Network', 'M2 12h4l3-8 4 16 3-8h6'],
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
    { path: '/players', component: () => import('./views/Players.vue') },
    { path: '/activity', component: () => import('./views/Playlists.vue') },
    { path: '/network', component: () => import('./views/Network.vue') },
    { path: '/updates', component: () => import('./views/Updates.vue') },
    { path: '/security', component: () => import('./views/Security.vue') },
    { path: '/audit', component: () => import('./views/Audit.vue') },
    { path: '/:rest(.*)*', redirect: '/overview' },
  ],
});
