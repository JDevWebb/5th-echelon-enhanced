<template>
  <div class="stat" :title="title">
    <b :class="tone">{{ value }}<small v-if="unit">{{ unit }}</small></b>
    <span>{{ label }}</span>
    <small v-if="sub">{{ sub }}</small>
    <slot />
  </div>
</template>

<script setup>
// One of the band's key numbers.
defineProps({ value: [String, Number], label: String, sub: String, unit: String, tone: String, title: String });
</script>
