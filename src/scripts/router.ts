/**
 * Vue Router instance.
 *
 * Routes come from `src/pages/**` via the `vue-router/vite` file-based-routing plugin, so adding a
 * page is adding a file. `definePage({ meta })` declares route meta at the page itself.
 *
 * There is no navigation guard and no authentication: this is a local desktop app, and the OS user
 * is the user.
 */
import { createRouter, createWebHistory } from 'vue-router';
import { routes } from 'vue-router/auto-routes';

export const router = createRouter({
  history: createWebHistory(),
  routes,
});
