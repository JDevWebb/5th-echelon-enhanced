<template>
  <div class="stack">
    <h2>Setup link for {{ who }}</h2>
    <p class="muted">Send it privately. It works once, within 24 hours, to choose a password and add a second factor.</p>
    <p class="secret">{{ link }}</p>
    <div class="row end">
      <button type="button" @click="copy">Copy</button>
      <button class="primary" type="button" @click="closeModal(true)">Done</button>
    </div>
  </div>
</template>

<script setup>
import { closeModal, toast } from '../lib/ui.js';
const props = defineProps({ who: String, link: String });
async function copy() {
  try { await navigator.clipboard.writeText(props.link); toast('Copied.'); } catch { toast('Select it by hand.', true); }
}
</script>
