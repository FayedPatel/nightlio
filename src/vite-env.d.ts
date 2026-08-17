/// <reference types="vite/client" />
/// <reference types="vite-plugin-pwa/client" />
// vite-plugin-pwa/client declares the `virtual:pwa-register` module imported
// by src/main.jsx (registerSW).

interface ImportMetaEnv {
  readonly VITE_API_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
