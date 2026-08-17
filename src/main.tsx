import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { registerSW } from 'virtual:pwa-register'
import './index.css'
import App from './App'
import { applyTransparentFavicon } from './utils/iconUtils'

// Try to remove the background from the favicon at runtime (non-destructive)
// No-op if it fails; keeps original favicon
applyTransparentFavicon({ src: '/logo.png', threshold: 0 }).catch(() => {});

// registerType is 'autoUpdate' (vite.config.ts) — a new service worker activates
// immediately (skipWaiting + clientsClaim in src/sw.ts) with no "reload to update"
// prompt. Activation alone does NOT refresh the page: the old bundle keeps running
// in already-open tabs, which left users on a stale login UI after deploys. When
// the new worker takes control it fires `controllerchange`; reload once so every
// open tab picks up the new shell. Guards: `hadController` skips the very first
// install (an uncontrolled page is already running the freshest build), and
// `reloaded` prevents reload loops if the event ever fires more than once.
if ('serviceWorker' in navigator) {
  const hadController = Boolean(navigator.serviceWorker.controller);
  let reloaded = false;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (!hadController || reloaded) return;
    reloaded = true;
    window.location.reload();
  });
}

registerSW({ immediate: true });

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
)
