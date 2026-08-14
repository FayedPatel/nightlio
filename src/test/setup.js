import '@testing-library/jest-dom/vitest';

// jsdom gaps: matchMedia (src/hooks/useMediaQuery.js) and ResizeObserver
// (recharts) don't exist there.
window.matchMedia ??= (query) => ({
  matches: false,
  media: query,
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
