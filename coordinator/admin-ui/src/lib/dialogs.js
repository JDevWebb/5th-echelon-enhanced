// Ready-made dialogs.
import Confirm from '../components/Confirm.vue';
import { openModal } from './ui.js';

/** A yes/no question in the page's own dialog. */
export const confirmBox = (title, body, yes = 'Confirm', danger = false) => openModal(Confirm, { title, body, yes, danger });
