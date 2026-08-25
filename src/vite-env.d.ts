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

// Compile-time constant injected via `define` in vite.config.ts (and mirrored
// in vitest.config.ts): the package.json version string, e.g. "0.6.0".
declare const __APP_VERSION__: string

