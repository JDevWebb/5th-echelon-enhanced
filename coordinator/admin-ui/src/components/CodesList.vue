<template>
  <div class="codes"><span v-for="c in codes" :key="c">{{ c }}</span></div>
</template>

<script setup>
defineProps({ codes: Array });
</script>
