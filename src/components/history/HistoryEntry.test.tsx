import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import type { ComponentProps } from 'react';
import HistoryEntry from './HistoryEntry';
import { ToastProvider } from '../ui/ToastProvider';
import type { MoodEntryWithSelections } from '../../hooks/useMoodData';

vi.mock('../../services/api', async () => {
  const { createMockApiService } = await import('../../test/mockApiService');
  return { default: createMockApiService() };
});

const entry: MoodEntryWithSelections = {
  id: 7,
  date: '2026-08-13',
  mood: 4,
  content: '# A good day\nWent for a walk.',
  created_at: '2026-08-13T18:00:00',
  // Real MoodEntry rows always carry updated_at; the old untyped mock
  // omitted it (unused by HistoryEntry, so tests behave identically).
  updated_at: '2026-08-13 18:00:00',
  selections: [],
};

const renderEntry = (props: Partial<ComponentProps<typeof HistoryEntry>> = {}) =>
  render(
    <ToastProvider>
      <HistoryEntry entry={entry} onDelete={vi.fn()} onEdit={vi.fn()} {...props} />
    </ToastProvider>,
  );

describe('HistoryEntry', () => {
  it('shows the entry date in display format for ISO-stored dates', () => {
    renderEntry();
    const display = new Date(2026, 7, 13).toLocaleDateString();
    expect(
      screen.getByLabelText(`Open entry from ${display}`),
    ).toBeInTheDocument();
  });

  it('fires onEdit with the entry when the edit action is used', () => {
    const onEdit = vi.fn();
    renderEntry({ onEdit });
    const display = new Date(2026, 7, 13).toLocaleDateString();
    fireEvent.click(screen.getByLabelText(`Edit entry from ${display}`));
    expect(onEdit).toHaveBeenCalledWith(entry);
  });
});
