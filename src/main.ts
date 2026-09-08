import { createPinia } from 'pinia';
import { createApp } from 'vue';
import App from '@/layout/App.vue';
import { router } from '@/scripts/router';

// The sentinel `tools/scripts/verify-prod-bundle.mjs` looks for. In a production build Vite
// replaces `import.meta.env.DEV` with `false` and the minifier removes this line entirely, taking
// the string with it. If the string survives into `dist/`, the bundle is a development build and
// every `import.meta.env.PROD` guard in the app is inverted. Do not remove it.
if (import.meta.env.DEV) console.info('__DEV_BUILD__');

const app = createApp(App);
app.use(createPinia());
app.use(router);

await router.isReady();
app.mount('body');
