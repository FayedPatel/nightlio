// helpers to split title/body and strip markdown for previews
export const stripMd = (s = '') => s
  .replace(/`{1,3}[^`]*`{1,3}/g, ' ')
  .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
  .replace(/\[(.*?)\]\([^)]*\)/g, '$1')
  .replace(/^#{1,6}\s+/gm, '')
  .replace(/^[>\-+*]\s+/gm, '')
  .replace(/[*_~`>#[\]()]/g, ' ')
  .replace(/\s+/g, ' ')
  .trim();

export const splitTitleBody = (content = ''): { title: string; body: string } => {
  const text = (content || '').replace(/\r\n/g, '\n').trim();
  if (!text) return { title: '', body: '' };
  const lines = text.split('\n');
  const first = (lines[0] || '').trim();
  const heading = first.match(/^#{1,6}\s+(.+?)\s*$/);
  if (heading) {
    return { title: (heading[1] ?? '').trim(), body: lines.slice(1).join('\n').trim() };
  }
  if (lines.length > 1) {
    return { title: first, body: lines.slice(1).join('\n').trim() };
  }
  const idx = first.indexOf(' ');
  if (idx > 0) {
    return { title: first.slice(0, idx).trim(), body: first.slice(idx + 1).trim() };
  }
  return { title: first, body: '' };
};

/** Plain-text entry title as the History card shows it; '' when there is none. */
export const entryTitle = (content = ''): string => stripMd(splitTitleBody(content).title).slice(0, 80);
