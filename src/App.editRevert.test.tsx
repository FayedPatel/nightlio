// Regression test for the editor revert: browsers structured-clone
// history.state, so React Router hands EntryView a fresh editingEntry object
// on every navigate (e.g. the replace navigate behind "Change mood"). The
// hydration effect, keyed on that object's identity, re-ran and reset the
// editor to the content the entry was opened with; burner toggles re-ran it
// too. The next autosave then wrote that stale text back to the server.
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { BrowserRouter } from 'react-router-dom';
import App from './App';
import apiService from './services/api';
import type { MarkdownAreaHandle } from './components/MarkdownArea';

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
vi.mock('./components/MarkdownArea', async () => {
  const { forwardRef, useImperativeHandle, useRef } = await import('react');
  const Editor = forwardRef<MarkdownAreaHandle, { initialMarkdown?: string; onChange?: (md: string) => void }>(
    ({ initialMarkdown, onChange }, ref) => {
      const area = useRef<HTMLTextAreaElement>(null);
      useImperativeHandle(ref, () => ({
        getMarkdown: () => area.current?.value ?? '',
        getInstance: () => ({
          setMarkdown: (md: string) => {
            if (area.current) area.current.value = md;
          },
        }),
      }));
      return (
        <textarea
          data-testid="editor"
          ref={area}
          defaultValue={initialMarkdown}
          onChange={(event) => onChange?.(event.target.value)}
        />
      );
    },
  );
  return { default: Editor };
});
vi.mock('./components/mood/MusicDock', () => ({ default: () => null }));

const typed = '# A good day\nWent for a walk, then wrote more.';
const { pushState, replaceState } = History.prototype;

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('nightlio_token', 'test-token');
  window.history.replaceState(null, '', '/dashboard/history');
  // jsdom stores history state by reference; browsers clone it.
  vi.spyOn(History.prototype, 'pushState').mockImplementation(function (this: History, data, unused, url) {
    pushState.call(this, structuredClone(data), unused, url);
  });
  vi.spyOn(History.prototype, 'replaceState').mockImplementation(function (this: History, data, unused, url) {
    replaceState.call(this, structuredClone(data), unused, url);
  });
  vi.mocked(apiService.updateMoodEntry).mockClear();
});

afterEach(() => {
  vi.mocked(History.prototype.pushState).mockRestore();
  vi.mocked(History.prototype.replaceState).mockRestore();
});

const openEditorAndType = async () => {
  render(
    <BrowserRouter>
      <App />
    </BrowserRouter>,
  );
  const display = new Date(2026, 7, 10).toLocaleDateString();
  fireEvent.click(await screen.findByLabelText(`Edit entry from ${display}`));
  const editor = await screen.findByTestId('editor');
  // Flush the hydration microtask; edits during it are ignored on purpose.
  await act(async () => {});
  fireEvent.change(editor, { target: { value: typed } });
  return editor;
};

describe('editing an existing entry', () => {
  it('keeps unsaved typing when the mood is changed', async () => {
    const editor = await openEditorAndType();

    fireEvent.click(screen.getByText('Change mood'));
    fireEvent.click(screen.getByTitle('Amazing'));

    expect(editor).toHaveValue(typed);
    await waitFor(
      () => expect(apiService.updateMoodEntry).toHaveBeenLastCalledWith(
        7,
        expect.objectContaining({ mood: 5, content: typed }),
      ),
      { timeout: 3000 },
    );
  });

  it('keeps the editor text when burner mode is toggled', async () => {
    const editor = await openEditorAndType();

    fireEvent.click(screen.getByLabelText('Toggle burner mode'));

    expect(editor).toHaveValue(typed);
  });
});
