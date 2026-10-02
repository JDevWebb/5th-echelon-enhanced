<template>
  <div class="gate">
    <div class="card" :class="{ wide }">
      <Brand :sub="sub" />
      <slot />
    </div>
  </div>
</template>

<script setup>
import Brand from './Brand.vue';
defineProps({ sub: { type: String, default: 'Network operations' }, wide: Boolean });
</script>
