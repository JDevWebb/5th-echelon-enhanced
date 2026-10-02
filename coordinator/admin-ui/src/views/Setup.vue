<template>
  <Gate sub="Admin setup">
    <h1>Set up your sign-in</h1>
    <p class="muted">Choose a password (12 characters or more; a passphrase is best). Next you'll add a passkey or an authenticator app.</p>
    <form class="stack" @submit.prevent="submit">
      <label class="field"><span>Password</span><input v-model="password" type="password" autocomplete="new-password" minlength="12" required autofocus></label>
      <label class="field"><span>Again</span><input v-model="again" type="password" autocomplete="new-password" required></label>
      <button class="primary" type="submit">Continue</button>
    </form>
    <p class="err" role="alert">{{ err }}</p>
  </Gate>
</template>

<script setup>
import { ref } from 'vue';
import Gate from '../components/Gate.vue';
import { call } from '../lib/api.js';
import { session } from '../lib/session.js';

const password = ref('');
const again = ref('');
const err = ref('');
async function submit() {
  err.value = '';
  if (password.value !== again.value) { err.value = 'The passwords don\'t match.'; return; }
  try {
    await call('POST', '/setup', { token: session.setupToken, password: password.value });
    session.setupToken = null;
    session.info = { stage: 'enroll' };
    session.view = 'enroll';
  } catch (e) { err.value = e.message; }
}
</script>
