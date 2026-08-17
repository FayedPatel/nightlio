import type { CSSProperties } from 'react';

/**
 * Inline-style object that also accepts CSS custom properties
 * (`--anything: value`), which React.CSSProperties alone rejects.
 * Shared across the codebase so components never need to cast style
 * objects that set design-token variables inline.
 */
export type CSSVars = CSSProperties & Record<`--${string}`, string>;
