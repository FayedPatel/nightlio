// Settings' Recent activity: an entry session row shows its start – end
// range and links the entry's title to History, which pulses that card.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import App from './App';
import type apiService from './services/api';

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
          created_at: '2026-08-10T12:00:00',
          updated_at: '2026-08-10 12:20:00',
        },
      ]),
      getActivity: vi.fn<typeof apiService.getActivity>().mockResolvedValue({
        activities: [
          {
            id: 3,
            user_id: 1,
            event_type: 'entry_deleted',
            metadata: { entry_id: 99 },
            created_at: '2026-08-11 09:00:00',
          },
          {
            id: 2,
            user_id: 1,
            event_type: 'entry_created',
            metadata: { entry_id: 7, date: '2026-08-10', last_saved_at: '2026-08-10 12:20:00' },
            created_at: '2026-08-10 12:00:00',
          },
        ],
        next_cursor: null,
      }),
    }),
  };
});
vi.mock('./components/MarkdownArea', () => ({ default: () => null }));
vi.mock('./components/mood/MusicDock', () => ({ default: () => null }));

beforeEach(() => {
  localStorage.setItem('nightlio_token', 'test-token');
});

describe('recent activity entry links', () => {
  it('shows the session range and highlights the linked History card', async () => {
    render(
      <MemoryRouter initialEntries={['/dashboard/settings']}>
        <App />
      </MemoryRouter>,
    );

    const link = await screen.findByRole('link', { name: 'A good day' });
    const row = link.closest('li') as HTMLElement;
    expect(row).toHaveTextContent('Created a journal entry for 2026-08-10');
    const start = new Date('2026-08-10T12:00:00Z').toLocaleString();
    const end = new Date('2026-08-10T12:20:00Z').toLocaleTimeString();
    expect(row).toHaveTextContent(`${start} – ${end}`);

    // A deleted entry keeps its label but has nothing to link to.
    const deleted = screen.getByText('Deleted a journal entry').closest('li') as HTMLElement;
    expect(within(deleted).queryByRole('link')).toBeNull();

    fireEvent.click(link);
    const display = new Date(2026, 7, 10).toLocaleDateString();
    const card = await screen.findByLabelText(`Open entry from ${display}`);
    expect(card.closest('[data-entry-id="7"]')).toHaveClass('entry-card-row--highlight');
    await waitFor(() => expect(card).toHaveFocus());
  });
});
