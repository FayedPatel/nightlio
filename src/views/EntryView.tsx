import { useState, useRef, useEffect, useCallback } from 'react';
import {
  AlertCircle,
  ArrowLeft,
  CheckCircle2,
  Clock3,
  CloudOff,
  Loader2,
  Trash2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import MoodPicker from '../components/mood/MoodPicker';
import MoodDisplay from '../components/mood/MoodDisplay';
import GroupSelector from '../components/groups/GroupSelector';
import GroupManager from '../components/groups/GroupManager';
import MDArea from '../components/MarkdownArea';
import type { MarkdownAreaHandle } from '../components/MarkdownArea';
import apiService from '../services/api';
import { useToast } from '../components/ui/ToastProvider';
import { useBurner } from '../contexts/BurnerContext';
import { useI18n } from '../i18n';
import { formatEntryDate, todayISO, yesterdayISO } from '../utils/dateUtils';
import type {
  CreateMoodEntryRequest,
  EntrySelection,
  Group,
  MoodValue,
  UpdateMoodEntryRequest,
} from '../types/api';
import type { MoodEntryWithSelections } from '../hooks/useMoodData';

const AUTOSAVE_DEBOUNCE_MS = 1200;

type SaveState = 'idle' | 'dirty' | 'saving' | 'saved' | 'error' | 'disabled';

/**
 * Draft payload the autosave machinery keeps in latestPayloadRef. `mood` is
 * null only before a mood is picked; the autosave effect early-returns on
 * `!payload.mood` before scheduling any save, so payloads that reach the API
 * always carry a real mood.
 */
interface AutosavePayload {
  mood: number | null;
  content: string;
  selected_options: number[];
  /** New entries only — editing never sends a date. */
  date?: string;
}

/** Autosave fallbacks only know the selected option ids, not their names. */
type SelectionStub = Pick<EntrySelection, 'id'> & Partial<EntrySelection>;

/**
 * Partial entry EntryView reports upward after an autosave create/update.
 * Only `id` is guaranteed: the update fallback and create branches build
 * entries without updated_at (and with SelectionStub selections), which
 * App's upsertEntry merges into the full list.
 */
export interface EntryUpsert {
  id: number;
  mood?: number | null;
  content?: string;
  date?: string;
  created_at?: string;
  updated_at?: string;
  selections?: SelectionStub[];
}

export interface EntryUpdateOptions {
  navigateAfterSave?: boolean;
  refreshAfterSave?: boolean;
}

export interface EntryViewProps {
  selectedMood?: MoodValue | undefined;
  groups: Group[];
  onBack: () => void;
  onEntryDeleted: (entryId: number) => void;
  onCreateGroup: (name: string) => Promise<boolean>;
  onCreateOption: (groupId: number, name: string) => Promise<boolean>;
  onSelectMood: (moodValue: MoodValue) => void;
  editingEntry?: MoodEntryWithSelections | null | undefined;
  onEntryUpdated: (entry: EntryUpsert, options?: EntryUpdateOptions) => void;
  onEditMoodSelect: (moodValue: MoodValue) => void;
}

const normalizeSelectedOptions = (optionIds: number[] = []): number[] => (
  [...optionIds].map((id) => Number(id)).filter((id) => Number.isFinite(id)).sort((a, b) => a - b)
);

// `date` only participates for new entries (editing never sends a date, so
// its snapshots keep date: null); a date change must mark the draft dirty so
// autosave picks it up.
const buildSnapshot = ({ mood, content, selectedOptions, date = null }: {
  mood: number | null | undefined;
  content: string | null | undefined;
  selectedOptions: number[];
  date?: string | null;
}): string => (
  JSON.stringify({
    mood: mood ?? null,
    content: content ?? '',
    selected_options: normalizeSelectedOptions(selectedOptions),
    date,
  })
);

const EntryView = ({
  selectedMood,
  groups,
  onBack,
  onEntryDeleted,
  onCreateGroup,
  onCreateOption,
  onSelectMood,
  editingEntry = null,
  onEntryUpdated,
  onEditMoodSelect,
}: EntryViewProps) => {
  const isEditing = Boolean(editingEntry);
  const initialSelectionIds = editingEntry?.selections?.map((selection) => selection.id) ?? [];

  const [selectedOptions, setSelectedOptions] = useState<number[]>(initialSelectionIds);
  const [showMoodPicker, setShowMoodPicker] = useState(false);
  const [markdownContent, setMarkdownContent] = useState(editingEntry?.content ?? '');
  // New entries only: which local day the entry is journaled for. Editing an
  // existing entry keeps its stored date untouched.
  const [entryDate, setEntryDate] = useState(todayISO());
  const [activeEntryId, setActiveEntryId] = useState<number | null>(editingEntry?.id ?? null);
  const [saveState, setSaveState] = useState<SaveState>('idle');
  const [lastSavedAt, setLastSavedAt] = useState<Date | null>(null);
  const [saveErrorMessage, setSaveErrorMessage] = useState('');

  const markdownRef = useRef<MarkdownAreaHandle | null>(null);
  const autosaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const isHydratingEditorRef = useRef(false);
  const saveInFlightRef = useRef(false);
  const pendingSaveRef = useRef(false);
  const latestPayloadRef = useRef<{ payload: AutosavePayload; snapshot: string } | null>(null);
  const lastSavedSnapshotRef = useRef('');
  const activeEntryIdRef = useRef<number | null>(activeEntryId);
  const createdByAutosaveRef = useRef(false);
  const skipAutosaveFlushRef = useRef(false);
  const selectedMoodRef = useRef(selectedMood);

  const { show } = useToast();
  const { isBurnerMode } = useBurner();
  const { t } = useI18n();

  const clearAutosaveTimer = useCallback(() => {
    if (autosaveTimerRef.current) {
      clearTimeout(autosaveTimerRef.current);
      autosaveTimerRef.current = null;
    }
  }, []);

  useEffect(() => {
    activeEntryIdRef.current = activeEntryId;
  }, [activeEntryId]);

  // Mirror selectedMood into a ref (declared before the hydration effect so it
  // syncs first within a commit). The hydration effect reads the current mood
  // without listing it as a dependency: re-running that effect on every mood
  // change would wipe in-progress editor state.
  useEffect(() => {
    selectedMoodRef.current = selectedMood;
  }, [selectedMood]);

  useEffect(() => {
    if (isEditing && editingEntry) {
      createdByAutosaveRef.current = false;
      skipAutosaveFlushRef.current = false;

      const selectionIds = editingEntry.selections?.map((selection) => selection.id) ?? [];
      const content = editingEntry.content || '';

      setSelectedOptions(selectionIds);
      setActiveEntryId(editingEntry.id);
      setMarkdownContent(content);

      isHydratingEditorRef.current = true;
      const instance = markdownRef.current?.getInstance?.();
      if (instance && typeof instance.setMarkdown === 'function') {
        instance.setMarkdown(content);
      }
      queueMicrotask(() => {
        isHydratingEditorRef.current = false;
      });

      lastSavedSnapshotRef.current = buildSnapshot({
        mood: selectedMoodRef.current ?? editingEntry.mood,
        content,
        selectedOptions: selectionIds,
      });
      setLastSavedAt(new Date());
      setSaveState(isBurnerMode ? 'disabled' : 'saved');
      setSaveErrorMessage('');
      return;
    }

    setSelectedOptions([]);
    setActiveEntryId(null);
    setMarkdownContent('');
    setEntryDate(todayISO());
    createdByAutosaveRef.current = false;
    skipAutosaveFlushRef.current = false;

    isHydratingEditorRef.current = true;
    const instance = markdownRef.current?.getInstance?.();
    if (instance && typeof instance.setMarkdown === 'function') {
      instance.setMarkdown('');
    }
    queueMicrotask(() => {
      isHydratingEditorRef.current = false;
    });

    lastSavedSnapshotRef.current = buildSnapshot({
      mood: selectedMoodRef.current,
      content: '',
      selectedOptions: [],
      date: todayISO(),
    });
    setLastSavedAt(null);
    setSaveState(isBurnerMode ? 'disabled' : 'idle');
    setSaveErrorMessage('');
  }, [isEditing, editingEntry, isBurnerMode]);

  useEffect(() => {
    if (!isEditing) {
      setShowMoodPicker(false);
    }
  }, [isEditing]);

  useEffect(() => {
    if (isBurnerMode) {
      clearAutosaveTimer();
      setSaveState('disabled');
      return;
    }

    if (saveState === 'disabled') {
      setSaveState('idle');
    }
  }, [isBurnerMode, saveState, clearAutosaveTimer]);

  const executeAutosave = useCallback(
    async (payload: AutosavePayload, snapshot: string, silentError = false): Promise<boolean> => {
      if (isBurnerMode) return false;

      if (saveInFlightRef.current) {
        pendingSaveRef.current = true;
        return false;
      }

      saveInFlightRef.current = true;
      setSaveState('saving');
      setSaveErrorMessage('');

      try {
        if (activeEntryIdRef.current) {
          // Invariant cast: saves are only scheduled after the `!payload.mood`
          // gate in the autosave effect, so mood is a real MoodValue here.
          const response = await apiService.updateMoodEntry(
            activeEntryIdRef.current,
            payload as UpdateMoodEntryRequest,
          );
          const updatedEntry: EntryUpsert = response?.entry
            ? {
                ...response.entry,
                selections: response.entry.selections ?? [],
              }
            : {
                id: activeEntryIdRef.current,
                mood: payload.mood,
                content: payload.content,
                selections: normalizeSelectedOptions(payload.selected_options).map((id) => ({ id })),
              };

          if (typeof onEntryUpdated === 'function') {
            onEntryUpdated(updatedEntry, {
              navigateAfterSave: false,
              refreshAfterSave: false,
            });
          }
        } else {
          const now = new Date();
          const createPayload = {
            ...payload,
            date: payload.date || todayISO(),
            time: now.toISOString(),
          };

          // Same invariant cast as above: mood is non-null whenever a save
          // was scheduled.
          const response = await apiService.createMoodEntry(createPayload as CreateMoodEntryRequest);
          const newEntryId = response?.entry_id;

          if (newEntryId) {
            setActiveEntryId(newEntryId);
            activeEntryIdRef.current = newEntryId;
            createdByAutosaveRef.current = true;

            if (typeof onEntryUpdated === 'function') {
              onEntryUpdated(
                {
                  id: newEntryId,
                  mood: payload.mood,
                  content: payload.content,
                  date: createPayload.date,
                  created_at: createPayload.time,
                  selections: normalizeSelectedOptions(payload.selected_options).map((id) => ({ id })),
                },
                {
                  navigateAfterSave: false,
                  refreshAfterSave: true,
                }
              );
            }
          }

          if (response?.new_achievements?.length) {
            show(t('toast.savedAchievements'), 'success');
          }
        }

        lastSavedSnapshotRef.current = snapshot;
        setLastSavedAt(new Date());
        setSaveState('saved');
        return true;
      } catch (error) {
        console.error('Autosave failed:', error);
        setSaveState('error');
        setSaveErrorMessage(t('entry.autosaveFailedRetry'));
        if (!silentError) {
          show(t('toast.autosaveFailed'), 'error');
        }
        return false;
      } finally {
        saveInFlightRef.current = false;

        if (pendingSaveRef.current) {
          pendingSaveRef.current = false;
          const latest = latestPayloadRef.current;
          if (latest && latest.snapshot !== lastSavedSnapshotRef.current) {
            void executeAutosave(latest.payload, latest.snapshot, true);
          }
        }
      }
    },
    [isBurnerMode, onEntryUpdated, show, t]
  );

  const flushPendingSave = useCallback(async (): Promise<boolean> => {
    if (skipAutosaveFlushRef.current) return true;

    clearAutosaveTimer();

    const latest = latestPayloadRef.current;
    if (!latest) return true;
    if (latest.snapshot === lastSavedSnapshotRef.current) return true;

    return executeAutosave(latest.payload, latest.snapshot);
  }, [clearAutosaveTimer, executeAutosave]);

  useEffect(() => {
    return () => {
      clearAutosaveTimer();
    };
  }, [clearAutosaveTimer]);

  useEffect(() => {
    return () => {
      if (!skipAutosaveFlushRef.current) {
        void flushPendingSave();
      }
    };
  }, [flushPendingSave]);

  useEffect(() => {
    if (isBurnerMode) return;

    const handleVisibilityChange = () => {
      if (document.visibilityState === 'hidden') {
        void flushPendingSave();
      }
    };

    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (saveState === 'dirty' || saveState === 'saving') {
        event.preventDefault();
        event.returnValue = '';
      }
    };

    document.addEventListener('visibilitychange', handleVisibilityChange);
    window.addEventListener('beforeunload', handleBeforeUnload);

    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('beforeunload', handleBeforeUnload);
    };
  }, [flushPendingSave, isBurnerMode, saveState]);

  useEffect(() => {
    clearAutosaveTimer();

    const payload: AutosavePayload = {
      mood: selectedMood ? Number(selectedMood) : null,
      content: markdownContent || '',
      selected_options: normalizeSelectedOptions(selectedOptions),
    };
    if (!isEditing) {
      // Also sent on updates of an autosave-created row, so changing the
      // date after the first save corrects the stored date.
      payload.date = entryDate;
    }
    const snapshot = buildSnapshot({
      mood: payload.mood,
      content: payload.content,
      selectedOptions: payload.selected_options,
      date: isEditing ? null : entryDate,
    });

    latestPayloadRef.current = { payload, snapshot };

    if (isBurnerMode) {
      setSaveState('disabled');
      return;
    }

    if (!payload.mood) {
      setSaveState('idle');
      return;
    }

    const trimmed = payload.content.trim();
    const hasMeaningfulContent = Boolean(trimmed);

    if (!hasMeaningfulContent) {
      setSaveState(activeEntryIdRef.current ? 'saved' : 'idle');
      return;
    }

    if (snapshot === lastSavedSnapshotRef.current) {
      if (!saveInFlightRef.current) {
        setSaveState('saved');
      }
      return;
    }

    setSaveState('dirty');

    autosaveTimerRef.current = setTimeout(() => {
      const latest = latestPayloadRef.current;
      if (!latest) return;
      if (latest.snapshot === lastSavedSnapshotRef.current) return;
      void executeAutosave(latest.payload, latest.snapshot);
    }, AUTOSAVE_DEBOUNCE_MS);

    return () => {
      clearAutosaveTimer();
    };
  }, [
    selectedMood,
    selectedOptions,
    markdownContent,
    entryDate,
    isEditing,
    isBurnerMode,
    executeAutosave,
    clearAutosaveTimer,
  ]);

  const handleOptionToggle = (optionId: number) => {
    setSelectedOptions((prev) => (
      prev.includes(optionId) ? prev.filter((id) => id !== optionId) : [...prev, optionId]
    ));
  };

  const handleMoodSelection = (moodValue: MoodValue) => {
    if (isEditing) {
      if (typeof onEditMoodSelect === 'function') {
        onEditMoodSelect(moodValue);
      }
      setShowMoodPicker(false);
    } else if (typeof onSelectMood === 'function') {
      onSelectMood(moodValue);
    }
  };

  const handleEditorChange = (nextMarkdown: string) => {
    if (isHydratingEditorRef.current) return;
    setMarkdownContent(nextMarkdown || '');
  };

  const resetDraftComposer = () => {
    isHydratingEditorRef.current = true;
    markdownRef.current?.getInstance?.()?.setMarkdown('');
    queueMicrotask(() => {
      isHydratingEditorRef.current = false;
    });

    setMarkdownContent('');
    setSelectedOptions([]);
    setShowMoodPicker(false);
    setSaveErrorMessage('');
    setSaveState('disabled');

    const resetSnapshot = buildSnapshot({
      mood: selectedMood,
      content: '',
      selectedOptions: [],
      date: entryDate,
    });
    lastSavedSnapshotRef.current = resetSnapshot;
    latestPayloadRef.current = {
      payload: {
        mood: selectedMood ? Number(selectedMood) : null,
        content: '',
        selected_options: [],
        date: entryDate,
      },
      snapshot: resetSnapshot,
    };
  };

  // Back/cancel KEEPS the entry. An earlier design deleted autosave-created
  // rows here ("Draft discarded" on back), which silently destroyed work the
  // status pill had just called "Saved" — the user-facing effect was entries
  // vanishing. Leaving now flushes any pending change via the unmount
  // flushPendingSave effect; destroying a draft is only ever the explicit
  // handleDiscard below.
  const handleCancel = () => {
    if (isBurnerMode && !isEditing) {
      clearAutosaveTimer();
      skipAutosaveFlushRef.current = true;
      resetDraftComposer();
    }

    if (typeof onBack === 'function') {
      onBack();
    }
  };

  const handleDiscard = async () => {
    const confirmed = window.confirm(t('entry.discardConfirm'));
    if (!confirmed) return;

    clearAutosaveTimer();
    skipAutosaveFlushRef.current = true;

    if (createdByAutosaveRef.current && activeEntryIdRef.current) {
      try {
        const draftId = activeEntryIdRef.current;
        await apiService.deleteMoodEntry(draftId);
        if (typeof onEntryDeleted === 'function') {
          onEntryDeleted(draftId);
        }
        show(t('toast.draftDiscarded'), 'success');
      } catch (error) {
        console.error('Failed to discard autosaved draft:', error);
        skipAutosaveFlushRef.current = false;
        show(t('toast.discardFailed'), 'error');
        return;
      }
    }

    if (typeof onBack === 'function') {
      onBack();
    }
  };

  const saveStatusMeta: { label: string; Icon: LucideIcon } = (() => {
    if (isBurnerMode) {
      return {
        label: t('entry.saveStatus.burner'),
        Icon: CloudOff,
      };
    }

    if (saveState === 'saving') {
      return {
        label: t('entry.saveStatus.saving'),
        Icon: Loader2,
      };
    }

    if (saveState === 'dirty') {
      return {
        label: t('entry.saveStatus.dirty'),
        Icon: AlertCircle,
      };
    }

    if (saveState === 'error') {
      return {
        label: saveErrorMessage || t('entry.saveStatus.error'),
        Icon: AlertCircle,
      };
    }

    if (saveState === 'saved') {
      const timestamp = lastSavedAt
        ? lastSavedAt.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
        : '';
      return {
        label: timestamp ? t('entry.saveStatus.savedAt', { time: timestamp }) : t('entry.saveStatus.allSaved'),
        Icon: CheckCircle2,
      };
    }

    return {
      label: t('entry.saveStatus.idle'),
      Icon: Clock3,
    };
  })();

  if (!selectedMood && !isEditing) {
    return (
      <div className="entry-mood-prompt">
        <h3>
          {t('entry.pickMoodPrompt')}
        </h3>
        <MoodPicker onMoodSelect={handleMoodSelection} />
      </div>
    );
  }

  return (
    <div className="entry-container">
      <div className="entry-grid">
        <div className="entry-left">
          {isEditing && editingEntry && (
            <div className="entry-editing-note">
              {t('entry.editingEntryFrom')} <strong>{formatEntryDate(editingEntry.date)}</strong>
            </div>
          )}
          {!isEditing && (
            <div className="entry-date-section">
              <label className="entry-date-section__label" htmlFor="entry-date-input">
                {t('entry.dateLabel')}
              </label>
              <div className="entry-date-section__controls">
                <input
                  id="entry-date-input"
                  className="entry-date-section__input"
                  type="date"
                  value={entryDate}
                  max={todayISO()}
                  onChange={(event) => {
                    const next = event.target.value;
                    if (next && next <= todayISO()) {
                      setEntryDate(next);
                    }
                  }}
                />
                <button
                  type="button"
                  className={`entry-date-chip${entryDate === yesterdayISO() ? ' is-active' : ''}`}
                  onClick={() => setEntryDate(yesterdayISO())}
                >
                  {t('common.yesterday')}
                </button>
                {entryDate !== todayISO() && (
                  <button
                    type="button"
                    className="entry-date-chip"
                    onClick={() => setEntryDate(todayISO())}
                  >
                    {t('common.today')}
                  </button>
                )}
              </div>
            </div>
          )}
          <div className="entry-mood-section">
            <MoodDisplay moodValue={selectedMood ?? null} showLabel={false}>
              <div className="entry-mood-actions">
                <button
                  type="button"
                  className="entry-save-button entry-return-button"
                  onClick={handleCancel}
                  aria-label={t('entry.returnToDashboardAria')}
                  title={t('entry.returnToDashboardAria')}
                >
                  <ArrowLeft size={16} aria-hidden="true" />
                  <span>{t('entry.returnToDashboard')}</span>
                </button>

                {!isEditing && !isBurnerMode && (
                  <button
                    type="button"
                    className="entry-save-button"
                    onClick={handleDiscard}
                    aria-label={t('entry.discardAria')}
                    title={t('entry.discardAria')}
                  >
                    <Trash2 size={16} aria-hidden="true" />
                    <span>{t('entry.discard')}</span>
                  </button>
                )}

                {/*
                  INVARIANT: this status pill must always render whenever the
                  entry editor is on screen (mood picked, or editing an
                  existing entry) — never gate its presence behind a
                  condition. Autosave now runs unattended with no manual Save
                  button, so this passive pill is the only saved-state
                  feedback the user gets; a prior refactor (commit da498b5)
                  once left the editor with no save feedback at all when an
                  explicit save bar was removed without keeping this in
                  place. Do not remove the element.
                */}
                <div
                  className={`entry-autosave-status is-${saveState}`}
                  role="status"
                  aria-live="polite"
                >
                  <saveStatusMeta.Icon
                    size={16}
                    className={saveState === 'saving' ? 'is-spinning' : ''}
                    aria-hidden="true"
                  />
                  <span>{saveStatusMeta.label}</span>
                </div>
              </div>
            </MoodDisplay>
            {isEditing && (
              <button
                type="button"
                className="entry-change-mood-btn"
                onClick={() => setShowMoodPicker(true)}
              >
                {t('entry.changeMood')}
              </button>
            )}
          </div>
          {isEditing && showMoodPicker && (
            <div className="entry-mood-picker-panel">
              <p className="entry-mood-picker-panel__intro">
                {t('entry.pickNewMood')}
              </p>
              <MoodPicker onMoodSelect={handleMoodSelection} />
              <div className="entry-mood-picker-panel__footer">
                <button
                  type="button"
                  className="entry-mood-picker-panel__cancel"
                  onClick={() => setShowMoodPicker(false)}
                >
                  {t('common.cancel')}
                </button>
              </div>
            </div>
          )}
          <GroupSelector
            groups={groups}
            selectedOptions={selectedOptions}
            onOptionToggle={handleOptionToggle}
          />
          <div className="entry-group-manager">
            <GroupManager
              groups={groups}
              onCreateGroup={onCreateGroup}
              onCreateOption={onCreateOption}
            />
          </div>
        </div>

        <div className="entry-right">
          <MDArea
            ref={markdownRef}
            initialMarkdown={editingEntry?.content ?? ''}
            onChange={handleEditorChange}
          />
        </div>
      </div>
    </div>
  );
};

export default EntryView;
