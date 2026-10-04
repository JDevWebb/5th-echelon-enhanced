<template>
  <div class="shell">
    <aside class="side">
      <Brand />
      <nav class="nav" aria-label="Pages">
        <RouterLink v-for="[id, label, d] in ROUTES" :key="id" :to="`/${id}`" :title="label" :aria-current="current === id ? 'page' : null">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="d" /></svg>
          <span>{{ label }}</span>
          <b v-if="id === 'alerts' && alertCount" class="badge" :class="{ bad: alertsBad }" :aria-label="`${alertCount} open`">{{ alertCount }}</b>
          <b v-if="id === 'reports' && reportCount" class="badge info" :aria-label="`${reportCount} open`">{{ reportCount }}</b>
        </RouterLink>
      </nav>
      <div class="who">
        <span class="small muted">Signed in as</span>
        <b>{{ session.info?.username }}</b>
        <button class="ghost small" type="button" @click="session.signOut()">Sign out</button>
      </div>
    </aside>
    <main class="main">
      <RouterView v-slot="{ Component }">
        <component :is="Component" />
      </RouterView>
    </main>
  </div>
</template>

<script setup>
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import Brand from '../components/Brand.vue';
import { ROUTES } from '../router.js';
import { session } from '../lib/session.js';
import { live } from '../lib/live.js';

const route = useRoute();
const current = computed(() => route.path.split('/')[1] || 'overview');
const alertCount = computed(() => live.overview?.alerts?.length || 0);
const reportCount = computed(() => live.overview?.open_reports || 0);
const alertsBad = computed(() => (live.overview?.alerts || []).some(a => a.level === 'bad'));
</script>

<style scoped>
.badge { margin-left: auto; min-width: 20px; padding: 1px 6px; border-radius: 999px; background: var(--warn); color: #1a1205; font-size: 11px; text-align: center; }
.badge.bad { background: var(--bad); color: #fff; }
.badge.info { background: var(--info); color: #fff; }
</style>
