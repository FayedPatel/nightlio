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

// The browser only re-fetches sw.js on its own schedule (navigation +
// ~24h cap). Long-lived tabs (installed PWA left open) would otherwise not
// learn about a deploy for up to a day, so poll for a new worker hourly and
// whenever the tab becomes visible again. Once a new worker is found, the
// controllerchange handler above does the actual silent reload.
registerSW({
  immediate: true,
  onRegisteredSW(_url, registration) {
    if (!registration) return;
    const checkForUpdate = () => {
      registration.update().catch(() => {});
    };
    setInterval(checkForUpdate, 60 * 60 * 1000);
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') checkForUpdate();
    });
  },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
)
