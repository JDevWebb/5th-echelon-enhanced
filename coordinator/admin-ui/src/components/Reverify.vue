<template>
  <div class="stack">
    <h2>Confirm it's you</h2>
    <p class="muted">This change needs your second factor again.</p>
    <button v-if="session.info?.passkeys > 0 && passkeysWork()" class="primary" type="button" @click="passkey"><KeyIcon />Use your passkey</button>
    <form v-if="session.info?.totp" class="stack" @submit.prevent="totp">
      <label class="field"><span>Authenticator code</span><input v-model="code" class="code" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000000"></label>
      <button type="submit">Confirm</button>
    </form>
    <p class="err" role="alert">{{ err }}</p>
    <div class="row end"><button class="ghost" type="button" @click="closeModal(false)">Cancel</button></div>
  </div>
</template>

<script setup>
import { ref } from 'vue';
import KeyIcon from './KeyIcon.vue';
import { call } from '../lib/api.js';
import { passkeysWork, usePasskey, passkeyError } from '../lib/passkeys.js';
import { session } from '../lib/session.js';
import { closeModal } from '../lib/ui.js';

const code = ref('');
const err = ref('');
async function passkey() {
  try { await usePasskey(); closeModal(true); } catch (e) { err.value = passkeyError(e); }
}
async function totp() {
  try { await call('POST', '/login/totp', { code: code.value }); closeModal(true); } catch (e) { err.value = e.message; }
}
</script>
