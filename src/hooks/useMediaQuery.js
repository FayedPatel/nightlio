import { useEffect, useState } from 'react';

/**
 * Subscribes to a CSS media query via `window.matchMedia` and returns whether
 * it currently matches. Keeps JS-side responsive logic in sync with the CSS
 * breakpoint convention documented in `src/index.css` (640px mobile, 900px
 * tablet) instead of ad-hoc `window.innerWidth` checks.
 *
 * Guards against environments where `window`/`matchMedia` are unavailable
 * (SSR, tests) by defaulting to `false` and skipping the subscription.
 *
 * @param {string} query - a CSS media query string, e.g. '(max-width: 640px)'
 * @returns {boolean} whether the query currently matches
 */
export default function useMediaQuery(query) {
  const getMatches = () =>
    typeof window !== 'undefined' && typeof window.matchMedia === 'function'
      ? window.matchMedia(query).matches
      : false;

  const [matches, setMatches] = useState(getMatches);

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
      return undefined;
    }

    const mediaQueryList = window.matchMedia(query);
    const handleChange = (event) => setMatches(event.matches);

    // Query string may have changed between renders — resync immediately.
    setMatches(mediaQueryList.matches);

    if (typeof mediaQueryList.addEventListener === 'function') {
      mediaQueryList.addEventListener('change', handleChange);
      return () => mediaQueryList.removeEventListener('change', handleChange);
    }

    // Safari < 14 fallback (deprecated but still needed for older WebKit).
    mediaQueryList.addListener(handleChange);
    return () => mediaQueryList.removeListener(handleChange);
  }, [query]);

  return matches;
}
