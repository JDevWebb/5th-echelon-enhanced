<template>
  <Gate>
    <h1>Confirm it's you</h1>
    <p class="muted">Your password was right. Now your second factor.</p>
    <button v-if="info.passkeys > 0 && passkeysWork()" class="primary" type="button" @click="passkey"><KeyIcon />Use your passkey</button>
    <div v-if="info.passkeys > 0 && passkeysWork() && info.totp" class="divider">or</div>
    <form v-if="info.totp" class="stack" @submit.prevent="totp">
      <label class="field"><span>Code from your authenticator app</span><input ref="codeInput" v-model="code" class="code" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000000" pattern="[0-9 ]*"></label>
      <button type="submit">Verify</button>
    </form>
    <button class="ghost small" type="button" @click="showRecovery = !showRecovery">Lost your device? Use a recovery code</button>
    <form v-if="showRecovery" class="stack" @submit.prevent="recovery">
      <label class="field"><span>Recovery code</span><input v-model="recoveryCode" autocomplete="off" placeholder="xxxx-xxxx-xxxx"></label>
      <button type="submit">Use recovery code</button>
    </form>
    <p class="err" role="alert">{{ err }}</p>
    <button class="ghost small" type="button" @click="session.signOut()">Start over</button>
  </Gate>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import Gate from '../components/Gate.vue';
import KeyIcon from '../components/KeyIcon.vue';
import { call } from '../lib/api.js';
import { passkeysWork, usePasskey, passkeyError } from '../lib/passkeys.js';
import { session } from '../lib/session.js';

const info = computed(() => session.info || {});
const code = ref('');
const codeInput = ref(null);
const recoveryCode = ref('');
const showRecovery = ref(false);
const err = ref('');
onMounted(() => codeInput.value?.focus());

async function passkey() {
  err.value = '';
  try { await usePasskey(); session.boot(); } catch (e) { err.value = passkeyError(e); }
}
async function totp() {
  err.value = '';
  try {
    await call('POST', '/login/totp', { code: code.value });
    session.boot();
  } catch (e) {
    err.value = e.message;
    codeInput.value?.select();
    if (e.status === 401 && /again/.test(e.message)) session.boot();
  }
}
async function recovery() {
  err.value = '';
  try { await call('POST', '/login/recovery', { code: recoveryCode.value }); session.boot(); } catch (e) { err.value = e.message; }
}
</script>
