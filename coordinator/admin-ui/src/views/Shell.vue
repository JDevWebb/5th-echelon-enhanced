<template>
  <div class="shell">
    <aside class="side">
      <Brand />
      <nav class="nav" aria-label="Pages">
        <RouterLink v-for="[id, label, d] in ROUTES" :key="id" :to="`/${id}`" :title="label" :aria-current="current === id ? 'page' : null">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="d" /></svg>
          <span>{{ label }}</span>
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

const route = useRoute();
const current = computed(() => route.path.split('/')[1] || 'overview');
</script>
