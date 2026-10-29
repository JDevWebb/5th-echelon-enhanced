<template>
  <label class="switch-row" :class="{ disabled }" :title="title || null">
    <input type="checkbox" role="switch" class="switch" :checked="on" :disabled="disabled" @change="$emit('set', $event.target.checked)" />
    <span>{{ label }}</span>
  </label>
</template>

<script setup>
// A switch with its label, as the egui launcher's theme::switch.
defineProps({ on: Boolean, label: { type: String, default: '' }, disabled: Boolean, title: { type: String, default: '' } });
defineEmits(['set']);
</script>

<style scoped>
.switch-row { display: inline-flex; align-items: center; gap: 10px; cursor: pointer; }
.switch-row.disabled { opacity: .5; cursor: default; }
.switch { appearance: none; flex: none; position: relative; width: 34px; height: 20px; margin: 0; border-radius: 10px; background: var(--control); border: 1px solid var(--control-line); cursor: inherit; transition: background .2s, border-color .2s; }
.switch::after { content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: var(--soft); transition: transform .2s var(--ease), background .2s; }
.switch:checked { background: var(--accent); border-color: var(--accent); }
.switch:checked::after { transform: translateX(14px); background: var(--on-accent); }
</style>
