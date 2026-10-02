<template>
  <div class="stack">
    <h2>Add an authenticator app</h2>
    <TotpEnroller @done="closeModal(true)" />
    <div class="row end"><button class="ghost" type="button" @click="closeModal(false)">Cancel</button></div>
  </div>
</template>

<script setup>
import TotpEnroller from './TotpEnroller.vue';
import { closeModal } from '../lib/ui.js';
</script>
