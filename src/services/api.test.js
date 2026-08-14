import { afterEach, describe, expect, it, vi } from 'vitest';

const importFreshApi = async (viteApiUrl) => {
  vi.resetModules();
  if (viteApiUrl === undefined) {
    vi.stubEnv('VITE_API_URL', '');
  } else {
    vi.stubEnv('VITE_API_URL', viteApiUrl);
  }
  const module = await import('./api');
  return module.default;
};

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe('buildUrl', () => {
  it('uses relative paths when no base is configured', async () => {
    const api = await importFreshApi('');
    expect(api.buildUrl('/api/config')).toBe('/api/config');
    expect(api.buildUrl('api/config')).toBe('/api/config');
  });

  it('prefixes an absolute http(s) base', async () => {
    const api = await importFreshApi('https://api.example.com/');
    expect(api.buildUrl('/api/config')).toBe('https://api.example.com/api/config');
  });

  it('strips stray quotes injected by build-time env', async () => {
    const api = await importFreshApi('"https://api.example.com"');
    expect(api.buildUrl('/api/config')).toBe('https://api.example.com/api/config');
  });

  it('does not double-prefix a path base', async () => {
    const api = await importFreshApi('/api');
    expect(api.buildUrl('/api/config')).toBe('/api/config');
    expect(api.buildUrl('/health')).toBe('/api/health');
  });

  it('rejects a base with a non-http(s) scheme and falls back to relative', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const api = await importFreshApi('file:///srv/app');
    expect(api.buildUrl('/api/config')).toBe('/api/config');
    expect(warn).toHaveBeenCalled();
  });
});
