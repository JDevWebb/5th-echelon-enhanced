<template>
  <div class="brand">
    <span class="mark"><i></i><i></i><i></i><i></i><i></i></span>
    <div><b>SCBL Network</b><span>{{ sub }}</span></div>
  </div>
</template>

<script setup>
defineProps({ sub: { type: String, default: 'Network operations' } });
</script>
