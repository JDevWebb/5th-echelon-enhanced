<template>
  <Gate sub="Admin setup">
    <h1>Add a second factor</h1>
    <p class="muted">Every admin signs in with a password and a second factor. A passkey (Touch ID, Windows Hello, a security key or your phone) is the strongest, and signs you in on its own.</p>
    <TotpEnroller v-if="useTotp" @done="finished" />
    <div v-else class="stack">
      <button v-if="passkeysWork()" class="primary" type="button" @click="passkey"><KeyIcon />Add a passkey (recommended)</button>
      <button type="button" @click="useTotp = true">Use an authenticator app instead</button>
    </div>
    <p class="err" role="alert">{{ err }}</p>
  </Gate>
</template>

<script setup>
import { ref } from 'vue';
import Gate from '../components/Gate.vue';
import KeyIcon from '../components/KeyIcon.vue';
import TotpEnroller from '../components/TotpEnroller.vue';
import { addPasskey, passkeysWork, passkeyError } from '../lib/passkeys.js';
import { session } from '../lib/session.js';

const useTotp = ref(false);
const err = ref('');
function finished(r) {
  if (r?.recovery_codes?.length) session.showCodes(r.recovery_codes);
  else session.boot();
}
async function passkey() {
  err.value = '';
  try { finished(await addPasskey()); } catch (e) { err.value = passkeyError(e); }
}
</script>
