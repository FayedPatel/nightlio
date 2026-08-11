// Custom service worker source for vite-plugin-pwa (injectManifest strategy).
//
// Hard product constraint: /api/* responses are never cached. Journal entries and
// auth state must always come straight from the server — no intermediary, ever,
// even the service worker's own cache. This file therefore:
//   1. Precaches only the built app shell (JS/CSS/HTML/icons/manifest) — static,
//      non-sensitive assets — via workbox-precaching.
//   2. Routes every /api/* request through NetworkOnly: no cache read, no cache
//      write, request always hits the network and fails loudly if the network
//      is unavailable (no stale auth/mood data ever served).
//   3. For page navigations, tries the network first; only if that fails (i.e.
//      genuinely offline) does it serve the static offline.html fallback. This
//      app has no offline data story (everything lives on the self-hosted
//      server), so there is no value in resurrecting a half-broken empty SPA
//      shell when offline — a clear "you're offline" page is more honest.

import { precacheAndRoute, cleanupOutdatedCaches } from 'workbox-precaching';
import { registerRoute } from 'workbox-routing';
import { NetworkOnly } from 'workbox-strategies';
import { clientsClaim } from 'workbox-core';

// Injected at build time with the list of built static assets to precache.
precacheAndRoute(self.__WB_MANIFEST);
cleanupOutdatedCaches();

// registerType: 'autoUpdate' — take over immediately, don't wait for old tabs
// to close, so users always get the newest shell without a manual refresh.
self.skipWaiting();
clientsClaim();

const OFFLINE_URL = '/offline.html';

// Never cache /api/* — always go to the network. This is intentionally the
// simplest possible strategy: no runtime cache, no stale-while-revalidate,
// nothing that could ever hand back a cached auth or journal response.
registerRoute(({ url }) => url.pathname.startsWith('/api/'), new NetworkOnly());

// Navigation requests: network-first, offline-page fallback. This is separate
// from the precached app shell — a plain `fetch` here means we always ask the
// network for the freshest HTML/routing first, and only reach for the offline
// fallback (precached above, since it lives in the build's public/ output)
// when the network is truly unreachable.
self.addEventListener('fetch', (event) => {
  if (event.request.mode !== 'navigate') return;

  event.respondWith(
    fetch(event.request).catch(() => caches.match(OFFLINE_URL)),
  );
});
