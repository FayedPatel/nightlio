// Regression test for backdated entries: the create payload must carry the
// user-chosen date (it used to hardcode today's toLocaleDateString()).
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import EntryView from './EntryView';
import { todayISO, yesterdayISO } from '../utils/dateUtils';

vi.mock('../services/api', async () => {
  const { createMockApiService } = await import('../test/mockApiService');
  return { default: createMockApiService() };
});
vi.mock('../components/MarkdownArea', () => ({
  default: ({ onChange }: { onChange: (markdown: string) => void }) => (
    <textarea
      data-testid="editor"
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
vi.mock('../components/ui/ToastProvider', () => ({
  useToast: () => ({ show: vi.fn() }),
}));
vi.mock('../contexts/BurnerContext', () => ({
  useBurner: () => ({ isBurnerMode: false }),
}));

import apiService from '../services/api';

const renderEntryView = async () => {
  const view = render(
    <EntryView
      selectedMood={4}
      groups={[]}
      onBack={vi.fn()}
      onEntryDeleted={vi.fn()}
      onCreateGroup={vi.fn()}
      onCreateOption={vi.fn()}
      onSelectMood={vi.fn()}
      onEntryUpdated={vi.fn()}
      onEditMoodSelect={vi.fn()}
    />,
  );
  // Flush the editor-hydration microtask; edits made while the hydration
  // guard is up are deliberately ignored by the component.
  await act(async () => {});
  return view;
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(apiService.createMoodEntry).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

const flushAutosave = async () => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
};

describe('EntryView entry date', () => {
  it('defaults new entries to today', async () => {
    await renderEntryView();
    fireEvent.change(screen.getByTestId('editor'), {
      target: { value: 'Wrote something meaningful.' },
    });
    await flushAutosave();

    expect(apiService.createMoodEntry).toHaveBeenCalledWith(
      expect.objectContaining({ date: todayISO() }),
    );
  });

  it('sends the chosen past date on create', async () => {
    await renderEntryView();
    fireEvent.click(screen.getByText('Yesterday'));
    fireEvent.change(screen.getByTestId('editor'), {
      target: { value: 'Forgot to journal yesterday.' },
    });
    await flushAutosave();

    expect(apiService.createMoodEntry).toHaveBeenCalledWith(
      expect.objectContaining({ date: yesterdayISO() }),
    );
  });

  it('blocks future dates in the date input', async () => {
    await renderEntryView();
    const input = screen.getByLabelText<HTMLInputElement>('Entry date');
    expect(input).toHaveAttribute('max', todayISO());

    const future = '2999-01-01';
    fireEvent.change(input, { target: { value: future } });
    expect(input.value).toBe(todayISO());
  });
});
