import { createApp } from 'vue';
import App from './App.vue';
import { vModal } from './lib/modal';
import './theme.css';

createApp(App).directive('modal', vModal).mount('#app');
