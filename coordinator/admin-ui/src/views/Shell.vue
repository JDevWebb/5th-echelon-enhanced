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
        </RouterLink>
      </template>
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

// The Roadmap page only where the coordinator keeps the roadmap (the community network's).
const hasRoadmap = ref(false);
onMounted(async () => {
  try {
    hasRoadmap.value = (await api('GET', '/me')).roadmap === true;
  } catch {
    // Left out: the page answers nothing without it.
  }
});
const rail = computed(() => ROUTES.filter(r => !r || r[0] !== 'roadmap' || hasRoadmap.value));

const route = useRoute();
const current = computed(() => {
  const page = route.path.split('/')[1] || 'overview';
  return PARENT[page] || page;
});
const alertCount = computed(() => live.overview?.alerts?.length || 0);
const reportCount = computed(() => live.overview?.open_reports || 0);
const alertsBad = computed(() => (live.overview?.alerts || []).some(a => a.level === 'bad'));
const version = computed(() => live.overview?.coordinator?.version || '');
</script>
