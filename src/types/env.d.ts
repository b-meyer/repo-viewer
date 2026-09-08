/// <reference types="vite-plus/client" />

declare module '*.vue' {
  import type { DefineComponent } from 'vue';
  // eslint-disable-next-line typescript/no-explicit-any -- Vue's own `*.vue` module shim uses `any` by design.
  const component: DefineComponent<any, any, any>;
  export default component;
}
