<template>
  <div class="top">
    <div>
      <h1>{{ title }}</h1>
      <p v-if="sub" class="sub">{{ sub }}</p>
    </div>
    <div class="row">
      <slot />
      <LiveBadge />
    </div>
  </div>
</template>

<script setup>
import LiveBadge from './LiveBadge.vue';
defineProps({ title: String, sub: String });
</script>
