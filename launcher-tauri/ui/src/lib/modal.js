// v-modal on a dialog: the keyboard stays in it while it's open. Focus moves in (to the dialog,
// so a screen reader reads its title, unless the dialog has focused a field of its own), Tab
// goes round inside it, Escape does what the directive's value says (left out where the dialog
// must be answered), and focus goes back where it was when the dialog closes. The topmost dialog
// open has the keyboard.
const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
const open = [];

const items = (el) => [...el.querySelectorAll(FOCUSABLE)].filter((i) => i.getClientRects().length > 0);

function keys(e) {
  const el = open.at(-1);
  if (!el) return;
  if (e.key === 'Escape' && el._escape) {
    e.preventDefault();
    e.stopPropagation();
    el._escape();
  } else if (e.key === 'Tab') {
    const all = items(el);
    const at = all.indexOf(document.activeElement);
    if (!all.length) {
      e.preventDefault();
      el.focus();
    } else if (at < 0 || (e.shiftKey && at === 0) || (!e.shiftKey && at === all.length - 1)) {
      e.preventDefault();
      (e.shiftKey ? all.at(-1) : all[0]).focus();
    }
  }
}

export const vModal = {
  mounted(el, { value }) {
    el._back = document.activeElement;
    el._escape = value;
    el.tabIndex = -1;
    if (!open.length) document.addEventListener('keydown', keys, true);
    open.push(el);
    requestAnimationFrame(() => {
      if (el.contains(document.activeElement)) return;
      el.focus({ preventScroll: true });
    });
  },
  updated(el, { value }) {
    el._escape = value;
  },
  unmounted(el) {
    open.splice(open.indexOf(el), 1);
    if (!open.length) document.removeEventListener('keydown', keys, true);
    if (el._back?.isConnected) el._back.focus({ preventScroll: true });
  },
};
