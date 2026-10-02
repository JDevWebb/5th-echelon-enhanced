<template>
  <div class="stack">
    <h2>Your new recovery codes</h2>
    <p class="muted">Save them now: they won't be shown again.</p>
    <CodesList :codes="codes" />
    <div class="row end"><button class="primary" type="button" @click="closeModal(true)">I've saved them</button></div>
  </div>
</template>

<script setup>
import CodesList from './CodesList.vue';
import { closeModal } from '../lib/ui.js';
defineProps({ codes: Array });
</script>
