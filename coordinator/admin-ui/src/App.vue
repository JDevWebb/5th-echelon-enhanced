<template>
  <div v-if="session.view === 'loading'" class="boot"><span class="mark"><i></i><i></i><i></i><i></i><i></i></span></div>
  <Login v-else-if="session.view === 'login'" />
  <SecondFactor v-else-if="session.view === 'password'" />
  <Setup v-else-if="session.view === 'setup'" />
  <Enroll v-else-if="session.view === 'enroll'" />
  <RecoveryCodes v-else-if="session.view === 'codes'" />
  <Shell v-else />

  <div v-if="toastState.shown" id="toast" :class="{ bad: toastState.bad }" role="status">{{ toastState.text }}</div>
  <dialog ref="dialog" @cancel.prevent="closeModal(false)" @close="modal.component && closeModal(false)">
    <component :is="modal.component" v-if="modal.component" v-bind="modal.props" @done="closeModal" />
  </dialog>
</template>

<script setup>
import { ref, watch } from 'vue';
import Login from './views/Login.vue';
import SecondFactor from './views/SecondFactor.vue';
import Setup from './views/Setup.vue';
import Enroll from './views/Enroll.vue';
import RecoveryCodes from './views/RecoveryCodes.vue';
import Shell from './views/Shell.vue';
import { session } from './lib/session.js';
import { connectLive, disconnectLive } from './lib/live.js';
import { closeModal, modal, toastState } from './lib/ui.js';

const dialog = ref(null);
watch(() => modal.component, c => {
  if (!dialog.value) return;
  if (c && !dialog.value.open) dialog.value.showModal();
  if (!c && dialog.value.open) dialog.value.close();
}, { flush: 'post' });

// Live while signed in, and only then.
watch(() => session.view, v => (v === 'full' ? connectLive() : disconnectLive()), { immediate: true });
</script>
