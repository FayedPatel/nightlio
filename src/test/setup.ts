import '@testing-library/jest-dom/vitest';

// jsdom gaps: matchMedia (src/hooks/useMediaQuery.ts) and ResizeObserver
// (recharts) don't exist there.
window.matchMedia ??= (query: string): MediaQueryList => ({
  matches: false,
  media: query,
  onchange: null,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
  dispatchEvent: () => false,
});

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
