<template>
  <div class="page-head">
    <header class="hero">
      <div>
        <span class="eyebrow">5th Echelon Enhanced · {{ network }}</span>
        <h1>{{ title }}</h1>
        <p v-if="sub" class="lead">{{ sub }}</p>
      </div>
      <div class="hero-side">
        <div class="who">
          <span class="avatar" aria-hidden="true">{{ (session.info?.username || '?').slice(0, 1) }}</span>
          <span>
            <b>{{ session.info?.username }}</b>
            <small>admin{{ session.info?.you?.country ? ` · ${session.info.you.country}` : '' }}</small>
          </span>
          <button class="ghost small" type="button" @click="session.signOut()">Sign out</button>
        </div>
        <LiveBadge />
      </div>
      <nav v-if="tabs" class="tabs" :aria-label="`${tabsLabel || title} pages`">
        <RouterLink v-for="[to, label] in tabs" :key="to" :to="to" :aria-current="route.path === to ? 'page' : null">{{ label }}</RouterLink>
      </nav>
    </header>
    <div v-if="$slots.stats || $slots.default" class="band">
      <slot name="stats" />
      <span class="spacer"></span>
      <div v-if="$slots.default" class="band-actions"><slot /></div>
    </div>
  </div>
</template>

<script setup>
// A page's head, as the launcher's: the hero (which network, the page's name, what it's
// for, who's signed in), then the band with the page's key numbers and its main actions.
import { useRoute } from 'vue-router';
import LiveBadge from './LiveBadge.vue';
import { session } from '../lib/session.js';

defineProps({ title: String, sub: String, tabs: Array, tabsLabel: String });
const route = useRoute();
// The network is the coordinator this admin UI belongs to, by the address it's reached at.
const network = location.hostname || 'this network';
</script>
