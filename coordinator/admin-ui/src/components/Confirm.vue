<template>
  <div class="stack">
    <h2>{{ title }}</h2>
    <p class="muted">{{ body }}</p>
    <div class="row end">
      <button class="ghost" type="button" @click="closeModal(false)">Cancel</button>
      <button :class="danger ? 'danger' : 'primary'" type="button" @click="closeModal(true)">{{ yes }}</button>
    </div>
  </div>
</template>

<script setup>
import { closeModal } from '../lib/ui.js';
defineProps({ title: String, body: String, yes: { type: String, default: 'Confirm' }, danger: Boolean });
</script>
