<template>
  <Transition name="modal">
    <div v-if="diagnostics || options.length" class="backdrop">
      <div class="dialog" role="dialog" aria-modal="true" :aria-labelledby="diagnostics ? 'q-diagnostics' : 'q-options'">
        <template v-if="diagnostics">
          <p class="kicker">Before you play</p>
          <h2 id="q-diagnostics">{{ diagnostics.title }}</h2>
          <div class="middle">
            <p class="soft">{{ diagnostics.what }}</p>
            <p class="soft">{{ diagnostics.private }}</p>
            <pre v-if="example" class="example">{{ example.length ? example.join('\n') : 'Nothing yet: your game hasn’t written a log here.' }}</pre>
            <p class="muted">{{ diagnostics.later }}</p>
          </div>
          <div class="actions">
            <button v-if="!example" type="button" class="btn" @click="showExample">Show me an example</button>
            <span class="grow"></span>
            <!-- The two answers alike: neither is the one to pick. -->
            <button type="button" class="btn" @click="answerDiagnostics(true)">Keep sending</button>
            <button type="button" class="btn" @click="answerDiagnostics(false)">Turn it off</button>
          </div>
        </template>

        <template v-else>
          <p class="kicker">Before you play</p>
          <h2 id="q-options">New game options</h2>
          <p class="soft">{{ intro }}</p>
          <div class="middle list">
            <label v-for="(o, i) in options" :key="o.id" class="option" :class="{ blocked: o.blocked }">
              <span class="head">
                <input v-model="choices[i]" type="checkbox" role="switch" class="switch" :disabled="!!o.blocked" />
                <b>{{ o.title }}</b>
                <span v-if="o.blocked" class="badge">{{ o.blocked[0] }}</span>
                <span v-else-if="o.recommended" class="badge good">Recommended</span>
              </span>
              <span class="soft">{{ o.blocked ? o.blocked[1] : o.text }}</span>
            </label>
            <p class="muted">{{ later }}</p>
          </div>
          <div class="actions">
            <span class="grow"></span>
            <button type="button" class="btn primary" @click="saveOptions">Save choices</button>
          </div>
        </template>
      </div>
    </div>
  </Transition>
</template>

<script setup>
// What the player answers once before the game starts (the egui launcher's diagnostics.rs and
// new_options.rs, with their wording from the shared library): whether the game may send its
// diagnostics, then the new game options. Not closed by a click beside it: the game waits.
import { onMounted, ref, watch } from 'vue';
import { call } from '../lib/tauri.js';

// The guided setup asks about the options in a step of its own (`withOptions` off meanwhile).
const props = defineProps({ withOptions: { type: Boolean, default: true } });
const emit = defineEmits(['answered', 'up']);

const diagnostics = ref(null);
const options = ref([]);
const choices = ref([]);
const intro = ref('');
const later = ref('');
const example = ref(null);

async function refresh() {
  const q = await call('questions');
  diagnostics.value = q.diagnostics;
  options.value = props.withOptions ? q.options : [];
  choices.value = options.value.map((o) => o.on);
  intro.value = q.options_intro;
  emit('up', Boolean(diagnostics.value || options.value.length));
  later.value = q.options_later;
}

async function showExample() {
  example.value = await call('diagnostics_example');
}

async function answerDiagnostics(send) {
  await call('answer_diagnostics', { send });
  example.value = null;
  await refresh();
  if (!options.value.length) emit('answered');
}

async function saveOptions() {
  await call('answer_options', { choices: choices.value });
  await refresh();
  emit('answered');
}

defineExpose({ refresh });
onMounted(refresh);
watch(() => props.withOptions, refresh);
</script>

<style scoped>
.backdrop { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; padding: 24px; background: rgba(3, 5, 6, .62); backdrop-filter: blur(6px); }
.dialog { width: min(600px, 100%); max-height: calc(100vh - 80px); display: flex; flex-direction: column; padding: 24px 26px 20px; border-radius: 6px; background: var(--surface); border: 1px solid var(--control-line); box-shadow: 0 24px 70px rgba(0, 0, 0, .6); }
h2 { margin: 2px 0 10px; font: 700 26px/1.1 PlexCond, sans-serif; text-transform: uppercase; letter-spacing: .01em; }
.middle { overflow: auto; min-height: 0; }
.middle p { margin: 0 0 10px; }
.example { margin: 4px 0 12px; padding: 10px 12px; max-height: 180px; overflow: auto; border-radius: 4px; background: var(--sunken); border: 1px solid var(--line); font: 11px/1.5 PlexMono, monospace; color: var(--soft); white-space: pre; user-select: text; }
.list { display: flex; flex-direction: column; gap: 8px; margin-top: 4px; }
.option { display: flex; flex-direction: column; gap: 4px; padding: 12px 16px; border-radius: 6px; background: var(--sunken); border: 1px solid var(--line); cursor: pointer; transition: border-color .15s; }
.option:hover { border-color: var(--control-line); }
.option.blocked { cursor: default; }
.head { display: flex; align-items: center; gap: 10px; }
.head b { font-weight: 600; }
.badge { margin-left: auto; padding: 1px 8px; border-radius: 4px; font: 10.5px PlexMono, monospace; letter-spacing: .08em; text-transform: uppercase; color: var(--warn); border: 1px solid rgba(240, 180, 76, .4); }
.badge.good { color: var(--accent); border-color: rgba(155, 224, 90, .4); }
.actions { display: flex; align-items: center; gap: 10px; margin-top: 16px; }
.grow { flex: 1; }
.switch { appearance: none; flex: none; position: relative; width: 34px; height: 20px; margin: 0; border-radius: 10px; background: var(--control); border: 1px solid var(--control-line); cursor: pointer; transition: background .2s, border-color .2s; }
.switch::after { content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: var(--soft); transition: transform .2s var(--ease), background .2s; }
.switch:checked { background: var(--accent); border-color: var(--accent); }
.switch:checked::after { transform: translateX(14px); background: var(--on-accent); }
.switch:disabled { opacity: .5; cursor: default; }
.modal-enter-active, .modal-leave-active { transition: opacity .2s; }
.modal-enter-active .dialog, .modal-leave-active .dialog { transition: transform .25s var(--ease); }
.modal-enter-from, .modal-leave-to { opacity: 0; }
.modal-enter-from .dialog { transform: translateY(10px) scale(.98); }
</style>
