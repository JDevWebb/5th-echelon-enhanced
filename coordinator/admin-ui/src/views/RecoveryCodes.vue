<template>
  <Gate>
    <h1>Save your recovery codes</h1>
    <p class="muted">If you lose your passkeys and authenticator, each of these signs you in once. Keep them somewhere safe, like a password manager. They won't be shown again.</p>
    <CodesList :codes="session.codes" />
    <div class="row"><button class="small" type="button" @click="copy">Copy</button></div>
    <label class="row"><input v-model="saved" type="checkbox" style="width: auto"><span>I've saved them</span></label>
    <button class="primary" type="button" :disabled="!saved" @click="session.boot()">Continue</button>
  </Gate>
</template>

<script setup>
import { ref } from 'vue';
import Gate from '../components/Gate.vue';
import CodesList from '../components/CodesList.vue';
import { session } from '../lib/session.js';
import { toast } from '../lib/ui.js';

const saved = ref(false);
async function copy() {
  try { await navigator.clipboard.writeText(session.codes.join('\n')); toast('Copied.'); } catch { toast('Couldn\'t copy: select them by hand.', true); }
}
</script>
