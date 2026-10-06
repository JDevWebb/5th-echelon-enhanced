// Light or dark: the admin's choice, or the system's (the default). Kept in this browser
// only; applied as data-theme on <html>, which styles.css keys its colours on.
import { ref } from 'vue';

const KEY = 'fes-theme';
export const THEMES = [
  ['system', 'System'],
  ['light', 'Light'],
  ['dark', 'Dark'],
];

const systemDark = window.matchMedia('(prefers-color-scheme: dark)');

function stored() {
  try {
    const v = localStorage.getItem(KEY);
    return THEMES.some(([id]) => id === v) ? v : 'system';
  } catch {
    // Storage blocked (a private window): the system's, each time.
    return 'system';
  }
}

/** The choice: system, light or dark. */
export const theme = ref(stored());

function apply() {
  const dark = theme.value === 'dark' || (theme.value === 'system' && systemDark.matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}

export function setTheme(value) {
  theme.value = THEMES.some(([id]) => id === value) ? value : 'system';
  try {
    localStorage.setItem(KEY, theme.value);
  } catch {
    // Not kept: it still applies until the page is closed.
  }
  apply();
}

/** The next choice, for a button that steps through them. */
export function nextTheme() {
  const i = THEMES.findIndex(([id]) => id === theme.value);
  setTheme(THEMES[(i + 1) % THEMES.length][0]);
}

// The system changing (evening, on a schedule) changes "System" at once.
systemDark.addEventListener('change', apply);
apply();
