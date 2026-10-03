<template>
  <div class="stack">
    <h2>Temporary password for {{ who }}</h2>
    <p class="muted">Give it to them privately (a direct message, not a channel). It's shown this once: the coordinator doesn't keep it. They should change it after signing in.</p>
    <p class="secret">{{ password }}</p>
    <div class="row end">
      <button type="button" @click="copy">Copy</button>
      <button class="primary" type="button" @click="closeModal(true)">Done</button>
    </div>
  </div>
</template>

<script setup>
import { closeModal, toast } from '../lib/ui.js';
const props = defineProps({ who: String, password: String });
async function copy() {
  try { await navigator.clipboard.writeText(props.password); toast('Copied.'); } catch { toast('Select it by hand.', true); }
}
</script>
