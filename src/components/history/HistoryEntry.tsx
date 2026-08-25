import { useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from 'react';
import { Pencil, Trash2 } from 'lucide-react';
import { getMoodIcon } from '../../utils/moodUtils';
import { formatEntryDate } from '../../utils/dateUtils';
import apiService from '../../services/api';
import { useToast } from '../ui/ToastProvider';
import type { MoodEntryWithSelections } from '../../hooks/useMoodData';
import EntryModal from './EntryModal';
import { useI18n } from '../../i18n';
import './HistoryEntry.css';

// How far the card slides to fully expose the edit/delete actions behind
// it — matches the actions strip width in HistoryEntry.css.
const REVEAL_WIDTH = 128;
// Net horizontal movement below this is treated as a tap, not a swipe.
const DRAG_THRESHOLD = 8;

// Phase 9c: cap visible tags instead of letting a heavily-tagged entry wrap
// across several rows — the featured (full-width) card gets a slightly
// higher cap than the narrower grid cards. Overflow collapses into a
// single "+n" chip (see .tag--overflow in App.css) rather than a truncated
// mid-wrap list.
const MAX_VISIBLE_TAGS_FEATURED = 6;
const MAX_VISIBLE_TAGS_GRID = 4;

interface HistoryEntryProps {
  entry: MoodEntryWithSelections;
  onDelete: (entryId: number) => void;
  onEdit?: ((entry: MoodEntryWithSelections) => void) | undefined;
  featured?: boolean;
}

interface SwipeState {
  active: boolean;
  pointerId: number | null;
  startX: number;
  startY: number;
  baseX: number;
  /** null until the gesture's axis is decided. */
  isHorizontal: boolean | null;
  dragged: boolean;
}

const HistoryEntry = ({ entry, onDelete, onEdit, featured = false }: HistoryEntryProps) => {
  const { t } = useI18n();
  const { icon: IconComponent, color } = getMoodIcon(entry.mood);
  const displayDate = formatEntryDate(entry.date);
  const [isDeleting, setIsDeleting] = useState(false);
  const [isHovered, setIsHovered] = useState(false);
  const [open, setOpen] = useState(false);

  // Touch-only swipe-to-reveal state. Pointer Events (not a library) —
  // same pattern as MusicDock's touch drag handling
  // (src/components/mood/MusicDock.tsx), adapted so vertical page
  // scrolling still passes through natively (touchAction: 'pan-y') while
  // horizontal drags are tracked in JS.
  const [swipeX, setSwipeX] = useState(0);
  const [isSwiping, setIsSwiping] = useState(false);
  const swipeState = useRef<SwipeState>({
    active: false,
    pointerId: null,
    startX: 0,
    startY: 0,
    baseX: 0,
    isHorizontal: null,
    dragged: false,
  });

  // helpers to split title/body and strip markdown for previews
  const stripMd = (s = '') => s
    .replace(/`{1,3}[^`]*`{1,3}/g, ' ')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[(.*?)\]\([^)]*\)/g, '$1')
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/^[>\-+*]\s+/gm, '')
    .replace(/[*_~`>#[\]()]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();

  const splitTitleBody = (content = ''): { title: string; body: string } => {
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

  const { title: rawTitle, body: rawBody } = splitTitleBody(entry.content || '');
  const title = stripMd(rawTitle).slice(0, 80);
  const excerpt = stripMd(rawBody).slice(0, 420);

  const { show } = useToast();
  const handleDelete = async (): Promise<boolean> => {
    if (!window.confirm(t('history.deleteConfirm'))) return false;
    setIsDeleting(true);
    try {
      await apiService.deleteMoodEntry(entry.id);
      onDelete(entry.id);
      show(t('toast.entryDeleted'), 'success');
      return true;
    } catch (error) {
      console.error('Failed to delete entry:', error);
      show(t('toast.deleteEntryFailed'), 'error');
      return false;
    } finally {
      setIsDeleting(false);
    }
  };

  const openPreview = () => setOpen(true);
  const onKey = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      openPreview();
    }
  };

  const handleEdit = () => {
    if (typeof onEdit === 'function') {
      onEdit(entry);
    }
  };

  // Desktop keeps the mouse-hover action toolbar (see HistoryEntry.css);
  // swipe-to-reveal is a touch-only affordance.
  const onCardPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.pointerType === 'mouse') return;
    swipeState.current = {
      active: true,
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      baseX: swipeX,
      isHorizontal: null,
      dragged: false,
    };
  };

  const onCardPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const s = swipeState.current;
    if (!s.active || e.pointerId !== s.pointerId) return;
    const dx = e.clientX - s.startX;
    const dy = e.clientY - s.startY;

    if (s.isHorizontal === null) {
      if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) return;
      s.isHorizontal = Math.abs(dx) > Math.abs(dy);
      if (s.isHorizontal && e.currentTarget.setPointerCapture) {
        e.currentTarget.setPointerCapture(e.pointerId);
      }
    }
    // Not a horizontal gesture — let touchAction: pan-y hand it to native
    // vertical scrolling instead of fighting it.
    if (!s.isHorizontal) return;

    s.dragged = true;
    setIsSwiping(true);
    const next = Math.max(-REVEAL_WIDTH, Math.min(0, s.baseX + dx));
    setSwipeX(next);
  };

  const endCardSwipe = (e: ReactPointerEvent<HTMLDivElement>) => {
    const s = swipeState.current;
    if (!s.active || (e && e.pointerId !== s.pointerId)) return;
    s.active = false;
    setIsSwiping(false);
    if (s.isHorizontal) {
      setSwipeX((cur) => (cur < -REVEAL_WIDTH / 2 ? -REVEAL_WIDTH : 0));
    }
  };

  const onCardClick = () => {
    const wasDragged = swipeState.current.dragged;
    swipeState.current.dragged = false;
    if (wasDragged) return; // this click was the tail end of a swipe, not a tap
    if (swipeX !== 0) {
      // Tapping the card while the actions panel is open closes it first,
      // matching common swipe-row UX, instead of opening the entry.
      setSwipeX(0);
      return;
    }
    openPreview();
  };

  const closeSwipeAndRun = (fn: () => void) => (e: ReactMouseEvent<HTMLButtonElement>) => {
    e.stopPropagation();
    setSwipeX(0);
    fn();
  };

  return (
    <div className="entry-card-row">
      <div
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={`entry-card${featured ? ' entry-card--featured' : ''}`}
        role="button"
        tabIndex={0}
        onClick={onCardClick}
        onKeyDown={onKey}
        onPointerDown={onCardPointerDown}
        onPointerMove={onCardPointerMove}
        onPointerUp={endCardSwipe}
        onPointerCancel={endCardSwipe}
        aria-label={t('history.openEntryAria', { date: displayDate })}
        style={{
          border: isHovered ? '1px solid color-mix(in oklab, var(--accent-600), transparent 55%)' : '1px solid var(--border)',
          boxShadow: isHovered ? 'var(--shadow-md)' : 'var(--shadow-sm)',
          cursor: 'pointer',
          outline: 'none',
          transform: swipeX ? `translateX(${swipeX}px)` : undefined,
          transition: isSwiping ? 'none' : 'transform 200ms cubic-bezier(.2,.6,.2,1)',
          touchAction: 'pan-y',
        }}
      >
        {/* Header: mood icon + date • time */}
        <div className="entry-card__header">
          <span style={{ color, display: 'flex', alignItems: 'center', justifyContent: 'center', width: 28, height: 28, borderRadius: '50%', background: 'var(--accent-bg-softer)', border: '1px solid var(--border)', flexShrink: 0 }}>
            <IconComponent size={18} strokeWidth={1.8} />
          </span>
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px', flexWrap: 'wrap' }}>
            <span style={{ fontWeight: 700, color: 'var(--text)' }}>{displayDate}</span>
            {entry.created_at && (
              <>
                <span aria-hidden="true" style={{ color: 'color-mix(in oklab, var(--text), transparent 40%)' }}>•</span>
                <span style={{ color: 'color-mix(in oklab, var(--text), transparent 20%)' }}>
                  {new Date(entry.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: true })}
                </span>
              </>
            )}
          </div>
        </div>

        {/* Title + excerpt preview */}
        <div className="entry-card__body">
          <div className="entry-card__title">{title || t('history.entryFallbackTitle')}</div>
          {excerpt && (
            <div className="entry-card__excerpt">{excerpt}</div>
          )}
        </div>

        {/* Tags: capped with a "+n" overflow chip instead of an unbounded
            wrap (Phase 9c) — see MAX_VISIBLE_TAGS_* above. */}
        {entry.selections && entry.selections.length > 0 && (() => {
          const maxVisible = featured ? MAX_VISIBLE_TAGS_FEATURED : MAX_VISIBLE_TAGS_GRID;
          const visible = entry.selections.slice(0, maxVisible);
          const hiddenCount = entry.selections.length - visible.length;
          return (
            <div className="entry-card__tags">
              {visible.map(selection => (
                <span key={selection.id} className="tag">{selection.name}</span>
              ))}
              {hiddenCount > 0 && (
                <span className="tag tag--overflow">+{hiddenCount}</span>
              )}
            </div>
          );
        })()}
      </div>

      {/* Edit/delete: revealed by swipe-left on touch, by hover/focus on
          pointer-capable devices (see HistoryEntry.css). */}
      <div className="entry-card-row__actions">
        <button
          type="button"
          className="entry-card__swipe-btn entry-card__swipe-btn--edit"
          onClick={closeSwipeAndRun(handleEdit)}
          disabled={!onEdit}
          aria-label={t('history.editEntryAria', { date: displayDate })}
        >
          <Pencil size={18} />
        </button>
        <button
          type="button"
          className="entry-card__swipe-btn entry-card__swipe-btn--delete"
          onClick={closeSwipeAndRun(handleDelete)}
          disabled={isDeleting}
          aria-label={t('history.deleteEntryAria', { date: displayDate })}
        >
          <Trash2 size={18} />
        </button>
      </div>

      {/* Modal for full view */}
      <EntryModal
        isOpen={open}
        entry={entry}
        onClose={() => setOpen(false)}
        onDelete={async () => {
          const ok = await handleDelete();
          if (ok) setOpen(false);
        }}
        isDeleting={isDeleting}
        onEdit={onEdit ? () => {
          setOpen(false);
          handleEdit();
        } : undefined}
      />
    </div>
  );
};

export default HistoryEntry;
