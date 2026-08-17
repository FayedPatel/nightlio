// Regression test for the edit-from-History blank screen: the edit action
// used relative navigate('entry'), which from /dashboard/history resolved to
// /dashboard/history/entry — no route matched and the nested <Routes>
// rendered nothing. Editing from the History page must land on
// /dashboard/entry with the editor mounted.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import App from './App';
import type apiService from './services/api';

const entry = {
  id: 7,
  date: '2026-08-10',
  mood: 4,
  content: '# A good day\nWent for a walk.',
  created_at: '2026-08-10T18:00:00',
};

vi.mock('./services/api', async () => {
  const { createMockApiService } = await import('./test/mockApiService');
  return {
    default: createMockApiService({
      getMoodEntries: vi.fn<typeof apiService.getMoodEntries>().mockResolvedValue([
        {
          id: 7,
          date: '2026-08-10',
          mood: 4,
          content: '# A good day\nWent for a walk.',
          created_at: '2026-08-10T18:00:00',
          updated_at: '2026-08-10 18:00:00',
        },
      ]),
    }),
  };
});
vi.mock('./components/MarkdownArea', () => ({
  default: ({ initialMarkdown }: { initialMarkdown?: string }) => (
    <textarea data-testid="editor" defaultValue={initialMarkdown} />
  ),
}));
vi.mock('./components/mood/MusicDock', () => ({ default: () => null }));

beforeEach(() => {
  localStorage.setItem('nightlio_token', 'test-token');
});

describe('editing an entry from the History page', () => {
  it('opens the entry editor instead of a blank screen', async () => {
    render(
      <MemoryRouter initialEntries={['/dashboard/history']}>
        <App />
      </MemoryRouter>,
    );

    const display = new Date(2026, 7, 10).toLocaleDateString();
    const editButton = await screen.findByLabelText(`Edit entry from ${display}`);
    fireEvent.click(editButton);

    // The editor view renders the editing note; with the old relative
    // navigation the nested Routes matched nothing and rendered null.
    expect(await screen.findByText(/Editing entry from/)).toBeInTheDocument();
    expect(screen.getByTestId('editor')).toHaveValue(entry.content);
  });
});
