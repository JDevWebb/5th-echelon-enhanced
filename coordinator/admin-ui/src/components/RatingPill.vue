<template>
  <span v-if="rating === 'bad'" class="pill bad">Bad</span>
  <span v-else-if="rating === 'good'" class="pill ok">Good</span>
  <span v-else class="muted">–</span>
</template>

<script setup>
defineProps({ rating: String });
</script>
