<template>
  <div class="shell">
    <nav class="sidenav" aria-label="Pages">
      <div class="side-brand">
        <span class="mark" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></span>
        <span class="word"><b>5th Echelon</b><span>Admin</span></span>
      </div>
      <template v-for="[group, items] in nav" :key="group || 'top'">
        <div v-if="group" class="side-group">{{ group }}</div>
        <RouterLink v-for="r in items" :key="r[0]" :to="`/${r[0]}`" class="side-link" :aria-current="current === r[0] ? 'page' : null">
          <svg viewBox="0 0 24 24" aria-hidden="true" v-html="r[2]"></svg>
          <span>{{ r[1] }}</span>
          <b v-if="badge(r[0])" class="count" :class="badge(r[0]).tone" :aria-label="badge(r[0]).label">{{ badge(r[0]).n }}</b>
        </RouterLink>
      </template>
      <div class="side-foot">
        <span v-if="version" class="mono">Coordinator {{ version }}</span>
        <span><i class="dot" :class="serversDown ? 'bad' : 'ok'"></i> {{ serversLine }}</span>
      </div>
    </nav>
    <div class="body">
      <header class="topbar">
        <form class="search" role="search" @submit.prevent="find">
          <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
          <input ref="searchBox" v-model="q" type="search" placeholder="Find a player by name, account # or identity" aria-label="Find a player">
          <kbd title="Press / to search">/</kbd>
        </form>
        <span class="online"><i class="dot ok"></i> {{ fmt.n(playing) }} playing now</span>
        <LiveBadge />
        <button type="button" class="ghost icon" :title="`Colours: ${themeLabel}. Click for ${nextLabel}.`" :aria-label="`Colours: ${themeLabel}; switch to ${nextLabel}`" @click="nextTheme">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <template v-if="theme === 'light'"><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></template>
            <path v-else-if="theme === 'dark'" d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />
            <template v-else><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></template>
          </svg>
        </button>
        <div class="me">
          <span class="avatar" aria-hidden="true">{{ (session.info?.username || '?').slice(0, 2) }}</span>
          <span class="me-name">{{ session.info?.username }}</span>
          <button class="ghost small" type="button" @click="session.signOut()">Sign out</button>
        </div>
      </header>
      <main class="main">
        <RouterView v-slot="{ Component }">
          <component :is="Component" />
        </RouterView>
      </main>
    </div>
  </div>
</template>

<script setup>
// The frame: the side menu in groups (the icons are router.js's own constants, hence v-html),
// the top bar (find a player, who's playing, live, colours, the admin), then the page.
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import LiveBadge from '../components/LiveBadge.vue';
import { NAV, PARENT } from '../router.js';
import { api } from '../lib/api.js';
import { live } from '../lib/live.js';
import { features } from '../lib/features.js';
import { fmt } from '../lib/fmt.js';
import { session } from '../lib/session.js';
import { THEMES, nextTheme, theme } from '../lib/theme.js';

// Colours: System, Light or Dark, a click stepping through them.
const label = id => THEMES.find(([t]) => t === id)?.[1] || 'System';
const themeLabel = computed(() => label(theme.value));
const nextLabel = computed(() => label(THEMES[(THEMES.findIndex(([t]) => t === theme.value) + 1) % THEMES.length][0]));

// The Roadmap and Support pages only where the coordinator keeps them (the community network's).
const hasRoadmap = ref(false);
onMounted(async () => {
  try {
    hasRoadmap.value = (await api('GET', '/me')).roadmap === true;
    features.roadmap = hasRoadmap.value;
  } catch {
    // Left out: the page answers nothing without it.
  }
});
const nav = computed(() => NAV
  .map(([group, items]) => [group, items.filter(r => !['roadmap', 'support'].includes(r[0]) || hasRoadmap.value)])
  .filter(([, items]) => items.length));

const route = useRoute();
const router = useRouter();
const current = computed(() => {
  const page = route.path.split('/')[1] || 'inbox';
  return PARENT[page] || page;
});

const o = computed(() => live.overview || {});
const alerts = computed(() => o.value.alerts || []);
// The Inbox's count: what's waiting on an admin (crashes to look at are on its page).
const waiting = computed(() => (o.value.support_open || 0) + (o.value.open_reports || 0) + (o.value.chat_reports || 0) + alerts.value.length);
function badge(id) {
  const n = {
    inbox: waiting.value,
    support: o.value.support_unread || 0,
    reports: o.value.open_reports || 0,
    moderation: o.value.chat_reports || 0,
    alerts: alerts.value.length,
  }[id] || 0;
  if (!n) return null;
  const tone = id === 'inbox' ? 'hot' : id === 'alerts' && alerts.value.some(a => a.level === 'bad') ? 'bad' : '';
  const what = { inbox: 'waiting on you', support: 'unread', reports: 'open', moderation: 'reported', alerts: 'open' }[id];
  return { n, tone, label: `${n} ${what}` };
}
const version = computed(() => o.value.coordinator?.version || '');
const servers = computed(() => o.value.servers || []);
const up = computed(() => servers.value.filter(sv => sv.online));
const serversDown = computed(() => servers.value.length - up.value.length);
const serversLine = computed(() => (!servers.value.length ? 'No servers yet' : serversDown.value ? `${serversDown.value} of ${servers.value.length} servers down` : `${servers.value.length} servers up`));
const playing = computed(() => up.value.reduce((n, sv) => n + (sv.metrics?.players?.online || 0), 0));

// Find a player: the Players page with the search filled in. "/" anywhere focuses it.
const q = ref('');
const searchBox = ref(null);
function find() {
  const text = q.value.trim();
  if (text) router.push({ path: '/players', query: { q: text } });
}
function slash(e) {
  if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey) return;
  const t = e.target;
  if (t && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName))) return;
  e.preventDefault();
  searchBox.value?.focus();
}
onMounted(() => window.addEventListener('keydown', slash));
onUnmounted(() => window.removeEventListener('keydown', slash));
</script>
