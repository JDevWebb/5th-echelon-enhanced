<template>
  <div class="shell">
    <nav class="rail" aria-label="Pages">
      <span class="mark" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></span>
      <template v-for="(r, i) in rail" :key="r ? r[0] : `sep-${i}`">
        <div v-if="!r" class="sep" aria-hidden="true"></div>
        <RouterLink v-else :to="`/${r[0]}`" class="nav" :aria-current="current === r[0] ? 'page' : null">
          <svg viewBox="0 0 24 24" aria-hidden="true" v-html="r[2]"></svg>
          <span>{{ r[1] }}</span>
          <b v-if="r[0] === 'alerts' && alertCount" class="badge" :class="{ bad: alertsBad }" :aria-label="`${alertCount} open`">{{ alertCount }}</b>
          <b v-if="r[0] === 'reports' && reportCount" class="badge info" :aria-label="`${reportCount} open`">{{ reportCount }}</b>
          <b v-if="r[0] === 'support' && supportCount" class="badge info" :aria-label="`${supportCount} unread`">{{ supportCount }}</b>
        </RouterLink>
      </template>
      <button type="button" class="theme" :title="`Colours: ${themeLabel}. Click for ${nextLabel}.`" :aria-label="`Colours: ${themeLabel}; switch to ${nextLabel}`" @click="nextTheme">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <template v-if="theme === 'light'"><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></template>
          <path v-else-if="theme === 'dark'" d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />
          <template v-else><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></template>
        </svg>
        <span>{{ themeLabel }}</span>
      </button>
      <span v-if="version" class="ver" :title="`Coordinator ${version}`">{{ version }}</span>
    </nav>
    <main class="main">
      <RouterView v-slot="{ Component }">
        <component :is="Component" />
      </RouterView>
    </main>
  </div>
</template>

<script setup>
// The frame: the rail (the icons are router.js's own constants, hence v-html), and the page.
import { computed, onMounted, ref } from 'vue';
import { useRoute } from 'vue-router';
import { PARENT, ROUTES } from '../router.js';
import { api } from '../lib/api.js';
import { live } from '../lib/live.js';
import { features } from '../lib/features.js';
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
const rail = computed(() => ROUTES.filter(r => !r || !['roadmap', 'support'].includes(r[0]) || hasRoadmap.value));

const route = useRoute();
const current = computed(() => {
  const page = route.path.split('/')[1] || 'overview';
  return PARENT[page] || page;
});
const alertCount = computed(() => live.overview?.alerts?.length || 0);
const reportCount = computed(() => live.overview?.open_reports || 0);
const supportCount = computed(() => live.overview?.support_unread || 0);
const alertsBad = computed(() => (live.overview?.alerts || []).some(a => a.level === 'bad'));
const version = computed(() => live.overview?.coordinator?.version || '');
</script>
