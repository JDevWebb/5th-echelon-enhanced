<template>
  <PageTop title="Security" sub="Your sign-in, the other admins, and where the admin UI may be reached from." />
  <template v-if="me && admins && rules">
    <div class="grid cols-2">
      <section class="panel">
        <header><h2>Your account</h2><span class="muted">{{ me.username }}</span></header>
        <div class="stack">
          <div>
            <span class="label">Passkeys</span>
            <table v-if="me.passkeys.length"><tbody>
              <tr v-for="p in me.passkeys" :key="p.id">
                <td>{{ p.name }}</td>
                <td class="small muted">added {{ fmt.when(p.created_at) }} · used {{ fmt.ago(p.last_used) }}</td>
                <td class="r"><button class="ghost small" type="button" @click="removePasskey(p)">Remove</button></td>
              </tr>
            </tbody></table>
            <p v-else class="muted small">None yet.</p>
            <button v-if="passkeysWork()" class="small" style="margin-top: 8px" type="button" @click="newPasskey"><KeyIcon />Add a passkey</button>
          </div>
          <div>
            <span class="label">Authenticator app</span>
            <div class="row" style="margin-top: 6px">
              <span class="pill" :class="{ ok: me.totp }">{{ me.totp ? 'on' : 'off' }}</span>
              <button v-if="me.totp" class="ghost small" type="button" @click="totpOff">Turn off</button>
              <button v-else class="small" type="button" @click="totpOn">Set up</button>
            </div>
          </div>
          <div>
            <span class="label">Recovery codes</span>
            <div class="row" style="margin-top: 6px">
              <span class="small">{{ me.recovery_left }} of 10 left</span>
              <button class="ghost small" type="button" @click="newCodes">Make new codes</button>
            </div>
          </div>
          <details>
            <summary class="small">Change password</summary>
            <form class="stack" style="margin-top: 10px" @submit.prevent="changePassword">
              <label class="field"><span>Current password</span><input v-model="pw.current" type="password" autocomplete="current-password"></label>
              <label class="field"><span>New password</span><input v-model="pw.new" type="password" autocomplete="new-password" minlength="12"></label>
              <div class="row end"><button type="submit">Change password</button></div>
            </form>
          </details>
        </div>
      </section>
      <section class="panel">
        <header><h2>Your sessions</h2></header>
        <div class="table-wrap"><table><tbody>
          <tr v-for="se in me.sessions" :key="se.id">
            <td><div><b v-if="se.current">This browser</b><span v-else>{{ browserName(se.user_agent) }}</span></div><div class="small muted">{{ se.ip }}{{ se.country ? ` · ${flag(se.country)} ${se.country}` : '' }}</div></td>
            <td class="small muted">active {{ fmt.ago(se.last_seen) }}</td>
            <td class="r"><button v-if="!se.current" class="ghost small" type="button" @click="endSession(se)">End</button></td>
          </tr>
        </tbody></table></div>
      </section>
    </div>
    <section class="panel">
      <header><h2>Where admins may sign in from</h2><span class="small muted">You: {{ rules.you.ip }}{{ rules.you.country ? ` · ${flag(rules.you.country)} ${rules.you.country}` : '' }}</span></header>
      <form class="grid cols-2" @submit.prevent="saveRules">
        <label class="field"><span>Networks (empty: any)</span><textarea v-model="nets" placeholder="One per line, e.g.&#10;203.0.113.7&#10;198.51.100.0/24&#10;2001:db8::/48"></textarea></label>
        <div class="stack">
          <label class="field"><span>Countries (empty: any)</span><input v-model="countries" placeholder="e.g. NZ, AU"></label>
          <p class="small muted">Countries come from Cloudflare. Rules that would lock you out are refused. Locked out anyway? On the coordinator's machine: coordinator admin open-access.</p>
          <div class="row end"><button class="primary" type="submit">Save rules</button></div>
        </div>
      </form>
    </section>
    <section class="panel">
      <header>
        <h2>Admins</h2>
        <form class="row" @submit.prevent="addAdmin"><input v-model="newName" placeholder="new admin's name" maxlength="32" style="max-width: 240px"><button class="small" type="submit">Add admin</button></form>
      </header>
      <div class="table-wrap"><table>
        <thead><tr><th>Admin</th><th>Second factors</th><th>Last sign-in</th><th></th></tr></thead>
        <tbody>
          <tr v-for="a in admins.admins" :key="a.id">
            <td>
              <b>{{ a.username }}</b><span v-if="a.username === me.username" class="faint"> (you)</span>
              <span v-if="a.disabled" class="pill bad" style="margin-left: 8px">disabled</span>
              <span v-if="!a.set_up" class="pill warn" style="margin-left: 8px">setup pending</span>
            </td>
            <td class="small"><span v-if="factors(a)">{{ factors(a) }}</span><span v-else class="faint">none</span></td>
            <td class="small muted">{{ fmt.ago(a.last_login) }}</td>
            <td class="r">
              <div v-if="a.username !== me.username" class="row end">
                <button class="ghost small" type="button" @click="adminAction(a, 'reset', [`Reset ${a.username}'s sign-in?`, 'Their password, passkeys and authenticator are cleared and they\'re signed out. You get a new setup link for them.', 'Reset sign-in'])">Reset sign-in</button>
                <button v-if="a.disabled" class="ghost small" type="button" @click="adminAction(a, 'enable')">Enable</button>
                <button v-else class="ghost small" type="button" @click="adminAction(a, 'disable', [`Disable ${a.username}?`, 'They\'re signed out and can\'t sign in until enabled.', 'Disable'])">Disable</button>
                <button class="danger small" type="button" @click="adminAction(a, 'remove', [`Remove ${a.username}?`, 'Their account is deleted.', 'Remove'])">Remove</button>
              </div>
            </td>
          </tr>
        </tbody>
      </table></div>
    </section>
  </template>
  <div v-else-if="error" class="panel"><p class="err">{{ error }}</p></div>
  <div v-else class="empty">Loading…</div>
</template>

<script setup>
import { onMounted, reactive, ref } from 'vue';
import PageTop from '../components/PageTop.vue';
import KeyIcon from '../components/KeyIcon.vue';
import TotpDialog from '../components/TotpDialog.vue';
import ShowCodes from '../components/ShowCodes.vue';
import SetupLink from '../components/SetupLink.vue';
import { api, call } from '../lib/api.js';
import { confirmBox } from '../lib/dialogs.js';
import { browserName, flag, fmt } from '../lib/fmt.js';
import { addPasskey, passkeyError, passkeysWork } from '../lib/passkeys.js';
import { session } from '../lib/session.js';
import { openModal, toast } from '../lib/ui.js';

const me = ref(null), admins = ref(null), rules = ref(null), error = ref('');
const nets = ref(''), countries = ref(''), newName = ref('');
const pw = reactive({ current: '', new: '' });

async function load() {
  try {
    [me.value, admins.value, rules.value] = await Promise.all([api('GET', '/me'), api('GET', '/admins'), api('GET', '/restrictions')]);
    nets.value = rules.value.networks.join('\n');
    countries.value = rules.value.countries.join(', ');
    // Its second factors may have changed: reverify needs them.
    session.info = { ...session.info, ...(await call('GET', '/session')) };
  } catch (e) { if (e.status !== 401) error.value = e.message; }
}
onMounted(load);

const factors = a => [a.passkeys ? `${a.passkeys} passkey${a.passkeys > 1 ? 's' : ''}` : null, a.totp ? 'authenticator' : null].filter(Boolean).join(' · ');
const run = async (fn, done) => { try { const r = await fn(); if (done) toast(typeof done === 'function' ? done(r) : done); load(); return r; } catch (e) { toast(e.message, true); } };

async function newPasskey() {
  try {
    const r = await addPasskey();
    toast(r.recovery_codes?.length ? 'Passkey added. New recovery codes were made.' : 'Passkey added.');
    load();
  } catch (e) { toast(passkeyError(e), true); }
}
async function removePasskey(p) {
  if (!await confirmBox(`Remove "${p.name}"?`, 'It won\'t sign you in any more.', 'Remove', true)) return;
  run(() => api('DELETE', `/me/passkeys/${encodeURIComponent(p.id)}`));
}
async function totpOff() {
  if (!await confirmBox('Turn off the authenticator app?', 'Your passkeys still sign you in.', 'Turn off', true)) return;
  run(() => api('DELETE', '/me/totp'));
}
async function totpOn() {
  if (await openModal(TotpDialog)) { toast('Authenticator app added.'); load(); }
}
async function newCodes() {
  if (!await confirmBox('Make new recovery codes?', 'The old ones stop working.', 'Make new codes')) return;
  try {
    const r = await api('POST', '/me/recovery', {});
    await openModal(ShowCodes, { codes: r.recovery_codes });
    load();
  } catch (e) { toast(e.message, true); }
}
async function changePassword() {
  await run(() => api('POST', '/me/password', { current: pw.current, new: pw.new }), 'Password changed. Your other sessions ended.');
  pw.current = pw.new = '';
}
const endSession = se => run(() => api('DELETE', `/sessions/${se.id}`));
const saveRules = () => run(() => api('PUT', '/restrictions', {
  networks: nets.value.split(/[\n,]+/).map(x => x.trim()).filter(Boolean),
  countries: countries.value.split(/[\s,]+/).filter(Boolean),
}), 'Saved. Requests from anywhere else are refused, the sign-in page too.');
async function addAdmin() {
  const name = newName.value.trim();
  try {
    const r = await api('POST', '/admins', { username: name });
    newName.value = '';
    await openModal(SetupLink, { who: name, link: r.link });
    load();
  } catch (e) { toast(e.message, true); }
}
async function adminAction(a, act, ask) {
  if (ask && !await confirmBox(ask[0], ask[1], ask[2], true)) return;
  try {
    const r = await api('POST', `/admins/${a.id}/${act}`, {});
    if (r.link) await openModal(SetupLink, { who: a.username, link: r.link });
    load();
  } catch (e) { toast(e.message, true); }
}
</script>
