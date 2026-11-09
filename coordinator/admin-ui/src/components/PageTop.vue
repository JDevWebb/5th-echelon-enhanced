<template>
  <div class="page-head">
    <div class="title-row">
      <div class="titles">
        <h1>{{ title }}</h1>
        <p v-if="sub" class="lead" :title="sub">{{ sub }}</p>
      </div>
      <div v-if="$slots.default" class="head-actions"><slot /></div>
    </div>
    <nav v-if="tabs" class="tabs" :aria-label="`${tabsLabel || title} pages`">
      <RouterLink v-for="[to, label] in tabs" :key="to" :to="to" :aria-current="route.path === to ? 'page' : null">{{ label }}</RouterLink>
    </nav>
    <div v-if="$slots.stats" class="band"><slot name="stats" /></div>
  </div>
</template>

<script setup>
// A page's head: its name and, in a line, what it's for; its main actions on the right; its
// tabs; then its key numbers as tiles. Who's signed in and the live state are in the top bar.
import { useRoute } from 'vue-router';

defineProps({ title: String, sub: String, tabs: Array, tabsLabel: String });
const route = useRoute();
</script>
