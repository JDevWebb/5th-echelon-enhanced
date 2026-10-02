<template>
  <div class="seg" role="group" aria-label="Period">
    <button v-for="[v, label] in options" :key="v" type="button" :aria-pressed="String(v === modelValue)" @click="$emit('update:modelValue', v)">{{ label }}</button>
  </div>
</template>

<script setup>
defineProps({ modelValue: Number, options: Array });
defineEmits(['update:modelValue']);
</script>
