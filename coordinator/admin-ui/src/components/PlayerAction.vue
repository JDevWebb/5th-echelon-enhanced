<template>
  <form class="stack" @submit.prevent="send">
    <h2>{{ title }}</h2>
    <p class="muted">{{ intro }}</p>
    <label v-if="kind === 'ban'" class="field"><span>Reason (the player sees it)</span><input ref="first" v-model="reason" maxlength="200" required placeholder="e.g. cheating"></label>
    <div v-if="kind === 'ban'" class="field">
      <span>For</span>
      <div class="seg" role="group" aria-label="How long">
        <button v-for="[secs, label] in DURATIONS" :key="label" type="button" :aria-pressed="String(secs === duration)" @click="duration = secs">{{ label }}</button>
      </div>
    </div>
    <label v-if="kind === 'rename'" class="field"><span>New name</span><input ref="first" v-model="name" maxlength="24" required pattern="[A-Za-z0-9_.\-]{3,24}" placeholder="3 to 24 letters, digits, _ - ."></label>
    <label v-if="kind === 'delete'" class="field"><span>Type <b>{{ player.name }}</b> to confirm</span><input ref="first" v-model="typed" autocomplete="off"></label>
    <label v-if="others && kind !== 'rename' && kind !== 'reset_password'" class="row"><input v-model="all" type="checkbox" style="width: auto"><span>All their accounts ({{ others + 1 }}, on every server)</span></label>
    <p class="err" role="alert">{{ err }}</p>
    <div class="row end">
      <button class="ghost" type="button" @click="closeModal(false)">Cancel</button>
      <button :class="danger ? 'danger' : 'primary'" type="submit" :disabled="busy || (kind === 'delete' && typed !== player.name)">{{ yes }}</button>
    </div>
  </form>
</template>

<script setup>
// Asks what's needed for one action on a player, and queues it. Answers the queued actions.
import { computed, onMounted, ref } from 'vue';
import { api } from '../lib/api.js';
import { closeModal } from '../lib/ui.js';

const props = defineProps({ kind: String, player: Object, server: String });
const DURATIONS = [[86400, '1 day'], [7 * 86400, '7 days'], [30 * 86400, '30 days'], [0, 'Permanent']];
const reason = ref('');
const duration = ref(7 * 86400);
const name = ref('');
const typed = ref('');
const all = ref(false);
const err = ref('');
const busy = ref(false);
const first = ref(null);
onMounted(() => first.value?.focus());
const others = computed(() => (props.player.identity ? props.player.also_on?.length || 0 : 0));
const danger = computed(() => props.kind === 'ban' || props.kind === 'delete');
const title = computed(() => ({
  ban: `Ban ${props.player.name}`, rename: `Rename ${props.player.name}`, delete: `Delete ${props.player.name}`,
  reset_password: `Reset ${props.player.name}'s password`, kick: `Kick ${props.player.name}`, unban: `Unban ${props.player.name}`,
})[props.kind]);
const intro = computed(() => ({
  ban: `They're signed out and can't sign in on ${props.server} until the ban ends.`,
  rename: 'They\'re signed out, and their friends on every server see the new name.',
  delete: `Their account on ${props.server} is removed for good: stats, friends and all. This can't be undone.`,
  reset_password: 'Their server makes a temporary password. You see it once, here, to give them privately.',
  kick: 'They\'re signed out. They can sign in again.',
  unban: 'They can sign in again.',
})[props.kind]);
const yes = computed(() => ({ ban: 'Ban', rename: 'Rename', delete: 'Delete for good', reset_password: 'Reset password', kick: 'Kick', unban: 'Unban' })[props.kind]);

async function send() {
  err.value = '';
  busy.value = true;
  const body = { kind: props.kind, all_servers: all.value };
  if (props.kind === 'ban') {
    body.reason = reason.value.trim();
    body.until = duration.value ? Math.floor(Date.now() / 1000) + duration.value : null;
  }
  if (props.kind === 'rename') body.name = name.value.trim();
  try {
    const r = await api('POST', `/players/${encodeURIComponent(props.player.server)}/${props.player.id}/actions`, body);
    closeModal(r.actions);
  } catch (e) {
    err.value = e.message;
  } finally {
    busy.value = false;
  }
}
</script>
