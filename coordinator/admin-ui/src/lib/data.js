// Loading page data, and keeping it current as the live connection says things changed.
import { ref, watch } from 'vue';
import { api } from './api.js';
import { live } from './live.js';

/** The overview: the live one, or fetched once if the connection hasn't sent it yet. */
export async function ensureOverview() {
  if (!live.overview) {
    live.overview = await api('GET', '/overview');
    live.at = Date.now();
  }
  return live.overview;
}

/** Runs `loader` now, again when `deps` change, and (with `live`) when servers report new
 * numbers, without blanking what's shown while it reloads. */
export function useLoad(loader, deps = () => null, { refresh = true } = {}) {
  const data = ref(null);
  const error = ref('');
  let seq = 0;
  async function load() {
    const mine = ++seq;
    try {
      const v = await loader();
      if (mine === seq) { data.value = v; error.value = ''; }
    } catch (e) {
      if (mine === seq && e.status !== 401) error.value = e.message;
    }
  }
  watch(deps, () => { data.value = null; load(); }, { immediate: true });
  if (refresh) watch(() => live.metricsTick, load);
  return { data, error, reload: load };
}
