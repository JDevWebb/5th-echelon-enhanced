<template>
  <div class="stack">
    <p v-if="!t && !failed" class="muted">Getting a code…</p>
    <p v-if="failed" class="err">{{ failed }}</p>
    <template v-if="t">
      <p class="muted">Scan this with your authenticator app (1Password, Bitwarden, Google Authenticator, Authy…), then enter the code it shows.</p>
      <!-- The server's QR code, shown as an image (an image can't run anything). -->
      <div class="qr"><img :src="qrSrc" alt="QR code for your authenticator app" width="204" height="204"></div>
      <details><summary class="small muted">Can't scan? Enter this key</summary><p class="secret">{{ t.secret.replace(/(.{4})/g, '$1 ').trim() }}</p></details>
      <form class="stack" @submit.prevent="confirm">
        <label class="field"><span>Code</span><input ref="input" v-model="code" class="code" inputmode="numeric" autocomplete="one-time-code" maxlength="7" placeholder="000000"></label>
        <button class="primary" type="submit">Confirm</button>
      </form>
      <p class="err" role="alert">{{ err }}</p>
    </template>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, ref } from 'vue';
import { api } from '../lib/api.js';

const emit = defineEmits(['done']);
const t = ref(null);
const failed = ref('');
const code = ref('');
const err = ref('');
const input = ref(null);
const qrSrc = computed(() => 'data:image/svg+xml;base64,' + btoa(t.value.qr));

onMounted(async () => {
  try {
    t.value = await api('POST', '/me/totp/begin', {});
    await nextTick();
    input.value?.focus();
  } catch (e) { failed.value = e.message; }
});
async function confirm() {
  err.value = '';
  try { emit('done', await api('POST', '/me/totp/confirm', { code: code.value })); } catch (e) { err.value = e.message; }
}
</script>
