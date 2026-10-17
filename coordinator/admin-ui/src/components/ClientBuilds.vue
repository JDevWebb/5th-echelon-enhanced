<template>
  <section class="panel">
    <header>
      <h2>Client builds</h2>
      <span class="small muted">Which launchers and game add-ons the network's servers take</span>
    </header>
    <div v-if="data" class="stack">
      <p class="small muted">Each launcher and game add-on sends a fingerprint of its own file when it signs in. The project's signed releases are known by theirs; anything else is a modified or development build, or a client older than 0.4.5. A client can be made to send a release's fingerprint, so this stops honest modified builds and casual tampering, not a determined attacker.</p>
      <div class="row">
        <label class="field" style="max-width: 320px"><span>Other builds</span>
          <select v-model="mode" @change="saveMode">
            <option value="off">Off: not checked</option>
            <option value="report">Report: noted on Sessions, still let in</option>
            <option value="enforce">Enforce: refused, unless allowed below</option>
          </select>
        </label>
        <p v-if="!data.builds.length" class="small" style="color: var(--warn)">No signed release recorded yet: servers judge nothing until the coordinator has seen one.</p>
      </div>
      <div>
        <span class="label">Allowed other builds (developers)</span>
        <table v-if="data.allowed.length"><tbody>
          <tr v-for="a in data.allowed" :key="a.identity">
            <td><b>{{ a.names.join(', ') || 'no account yet' }}</b><div class="small muted mono">{{ a.identity.slice(0, 12) }}…</div></td>
            <td class="small">{{ a.note }}</td>
            <td class="small muted">by {{ a.added_by }}, {{ fmt.ago(a.added_at) }}</td>
            <td class="r"><button class="ghost small" type="button" @click="remove(a)">Remove</button></td>
          </tr>
        </tbody></table>
        <p v-else class="muted small">Nobody. Add a developer from their player page (Allow other builds), or by identity here.</p>
        <form class="row" style="margin-top: 8px" @submit.prevent="add">
          <input v-model="identity" placeholder="identity (52 characters)" maxlength="52" style="max-width: 360px">
          <input v-model="note" placeholder="note, e.g. developing the mouse fix" maxlength="200" style="max-width: 280px">
          <button class="small" type="submit">Allow</button>
        </form>
      </div>
      <details>
        <summary class="small">Release builds known ({{ data.builds.length }})</summary>
        <div class="table-wrap"><table>
          <thead><tr><th>Release</th><th>File</th><th>SHA-256</th></tr></thead>
          <tbody><tr v-for="b in data.builds" :key="b.version + b.asset"><td>{{ b.version }}</td><td>{{ b.asset }}</td><td class="mono small">{{ b.sha256.slice(0, 16) }}…</td></tr></tbody>
        </table></div>
      </details>
    </div>
    <p v-else-if="error" class="err">{{ error }}</p>
  </section>
</template>

<script setup>
import { onMounted, ref } from 'vue';
import { api } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { fmt } from '../lib/fmt.js';
import { toast } from '../lib/ui.js';

const data = ref(null), error = ref(''), mode = ref('report'), identity = ref(''), note = ref('');

async function load() {
  try {
    data.value = await api('GET', '/clients');
    mode.value = data.value.mode;
  } catch (e) { if (e.status !== 401) error.value = e.message; }
}
onMounted(load);

async function saveMode() {
  if (mode.value === 'enforce' && !await confirmBox('Refuse other builds?', 'Players on a modified or development launcher or add-on, or on a client older than 0.4.5, can\'t sign in until they install a release, unless their identity is allowed. Check Sessions in report mode first.', 'Enforce', true)) {
    mode.value = data.value.mode;
    return;
  }
  try { await api('PUT', '/clients', { mode: mode.value }); toast('Saved: servers take it within a minute.'); load(); } catch (e) { toast(e.message, true); mode.value = data.value.mode; }
}
async function add() {
  try { await api('POST', '/clients/allowed', { identity: identity.value.trim(), note: note.value.trim() }); identity.value = note.value = ''; toast('Allowed: servers take it within a minute.'); load(); } catch (e) { toast(e.message, true); }
}
async function remove(a) {
  if (!await confirmBox(`Stop allowing ${a.names[0] || 'this identity'} other builds?`, 'With enforcement on, their next sign-in with another build is refused.', 'Remove', true)) return;
  try { await api('DELETE', `/clients/allowed/${encodeURIComponent(a.identity)}`); load(); } catch (e) { toast(e.message, true); }
}
</script>
