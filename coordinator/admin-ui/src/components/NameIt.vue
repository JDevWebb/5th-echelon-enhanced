<template>
  <form class="stack" @submit.prevent="save">
    <h2>Name {{ kind === 'map' ? 'map' : 'game mode' }} #{{ id }}</h2>
    <label class="field"><span>Name (empty removes it)</span><input ref="input" v-model="name" maxlength="48" :placeholder="kind === 'map' ? 'e.g. Penthouse' : 'e.g. Blacklist'"></label>
    <div class="row end">
      <button class="ghost" type="button" @click="closeModal(false)">Cancel</button>
      <button class="primary" type="submit">Save</button>
    </div>
  </form>
</template>

<script setup>
import { onMounted, ref } from 'vue';
import { api } from '../lib/api.js';
import { closeModal, toast } from '../lib/ui.js';

const props = defineProps({ kind: String, id: Number, current: String });
const name = ref(props.current || '');
const input = ref(null);
onMounted(() => input.value?.focus());
async function save() {
  try { await api('PUT', '/labels', { kind: props.kind, id: props.id, name: name.value }); closeModal(true); } catch (e) { toast(e.message, true); }
}
</script>
