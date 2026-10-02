<template>
  <Gate>
    <h1>Sign in</h1>
    <form class="stack" @submit.prevent="submit">
      <label class="field"><span>Admin name</span><input v-model="username" name="username" autocomplete="username webauthn" required autofocus></label>
      <label class="field"><span>Password</span><input ref="pass" v-model="password" name="password" type="password" autocomplete="current-password" required></label>
      <button class="primary" type="submit" :disabled="busy">Sign in</button>
    </form>
    <template v-if="passkeysWork()">
      <div class="divider">or</div>
      <button type="button" @click="passkey"><KeyIcon />Sign in with a passkey</button>
    </template>
    <p class="err" role="alert">{{ err }}</p>
    <p v-if="session.you" class="where">Connecting from {{ session.you.ip }}{{ session.you.country ? ' · ' + session.you.country : '' }}</p>
  </Gate>
</template>

<script setup>
import { ref } from 'vue';
import Gate from '../components/Gate.vue';
import KeyIcon from '../components/KeyIcon.vue';
import { call } from '../lib/api.js';
import { passkeysWork, usePasskey, passkeyError } from '../lib/passkeys.js';
import { session } from '../lib/session.js';

const username = ref('');
const password = ref('');
const pass = ref(null);
const err = ref('');
const busy = ref(false);

async function submit() {
  busy.value = true;
  err.value = '';
  try {
    const r = await call('POST', '/login', { username: username.value.trim(), password: password.value });
    session.info = { stage: r.stage, totp: r.totp, passkeys: r.passkeys };
    session.view = r.stage;
  } catch (e) {
    err.value = e.message;
    busy.value = false;
    pass.value?.select();
  }
}
async function passkey() {
  err.value = '';
  try { await usePasskey(); session.boot(); } catch (e) { err.value = passkeyError(e); }
}
</script>
