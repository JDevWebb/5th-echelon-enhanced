// Toasts and dialogs, shared by every page.
import { reactive, markRaw } from 'vue';

export const toastState = reactive({ text: '', bad: false, shown: false });
let toastTimer;

export function toast(text, bad = false) {
  Object.assign(toastState, { text, bad, shown: true });
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { toastState.shown = false; }, 4200);
}

/** The open dialog: a component, its props, and how to answer. One at a time. */
export const modal = reactive({ component: null, props: {}, resolve: null });

/** Opens `component` in the page's dialog; resolves with what it emits as `done` (or false). */
export function openModal(component, props = {}) {
  if (modal.resolve) modal.resolve(false);
  return new Promise(resolve => {
    modal.component = markRaw(component);
    modal.props = props;
    modal.resolve = resolve;
  });
}

export function closeModal(value = false) {
  const resolve = modal.resolve;
  modal.component = null;
  modal.props = {};
  modal.resolve = null;
  if (resolve) resolve(value);
}
