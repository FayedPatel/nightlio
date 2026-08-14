import ReactMarkdown from 'react-markdown';
import { useState } from 'react';
import { Pencil, Trash2, Download } from 'lucide-react';
import apiService from '../../services/api';
import { getMoodLabel } from '../../utils/moodUtils';
import { entryDateKey, formatEntryDate } from '../../utils/dateUtils';
import Modal from '../ui/Modal';
import './EntryModal.css';

const deriveTitleBody = (content = '') => {
  const text = (content || '').replace(/\r\n/g, '\n').trim();
  if (!text) return { title: '', body: '' };
  const lines = text.split('\n');
  const first = (lines[0] || '').trim();
  // If first line is a markdown heading like # Title
  const heading = first.match(/^#{1,6}\s+(.+?)\s*$/);
  if (heading) {
    return { title: heading[1].trim(), body: lines.slice(1).join('\n').trim() };
  }
  // Otherwise, multi-line: first line as title, remainder as body
  if (lines.length > 1) {
    return { title: first, body: lines.slice(1).join('\n').trim() };
  }
  // Single-line content; split at first space into title + body
  const idx = first.indexOf(' ');
  if (idx > 0) {
    return { title: first.slice(0, idx).trim(), body: first.slice(idx + 1).trim() };
  }
  // Single word only
  return { title: first, body: '' };
};

// Rebuilt on the shared Modal (Phase 9d) — was a bespoke fixed-position
// dialog before (centered on every viewport, no bottom-sheet, no
// safe-area, z-index: 100 outside the documented scale). Modal.jsx gives
// this the same sheet-on-mobile / swipe-dismiss / safe-area / z-index
// (var(--z-modal)) behavior every other dialog in the app already has.
const EntryModal = ({ isOpen, entry, onClose, onDelete, isDeleting, onEdit }) => {
  const [isExporting, setIsExporting] = useState(false);

  if (!isOpen || !entry) return null;

  const handleExport = async (e) => {
    e.stopPropagation();
    if (isExporting) return;
    setIsExporting(true);
    try {
      const timeStr = entry.created_at ? new Date(entry.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '';
      const dateStr = `${formatEntryDate(entry.date)}${timeStr ? ` at ${timeStr}` : ''}`;

      const moodLabel = entry.mood ? getMoodLabel(entry.mood) : '';

      const tagsStr = entry.selections?.length > 0
        ? entry.selections.map(s => s.name).join(', ')
        : '';

      const headerLines = [];
      headerLines.push(`**Date:** ${dateStr}`);
      if (moodLabel) {
        headerLines.push(`**Mood:** ${moodLabel}`);
      }
      if (tagsStr) {
        headerLines.push(`**Tags:** ${tagsStr}`);
      }

      const enhancedContent = headerLines.join('\n') + '\n\n---\n\n' + (entry.content || '');

      const blob = await apiService.exportPdf(enhancedContent);
      const url = window.URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `Entry_${entryDateKey(entry.date) || 'Export'}.pdf`;
      document.body.appendChild(a);
      a.click();
      window.URL.revokeObjectURL(url);
      document.body.removeChild(a);
    } catch (err) {
      console.error("Export error:", err);
      alert("Failed to export PDF.");
    } finally {
      setIsExporting(false);
    }
  };

  const { title, body } = deriveTitleBody(entry.content);
  const timeStr = entry.created_at
    ? new Date(entry.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    : '';

  const modalTitle = (
    <span className="entry-modal-title">
      <span className="entry-modal-title__date">{formatEntryDate(entry.date)}</span>
      {timeStr && <span className="entry-modal-title__time">{timeStr}</span>}
    </span>
  );

  const headerActions = (
    <>
      <button
        type="button"
        onClick={handleExport}
        disabled={isExporting}
        className="entry-modal-action-btn"
        title="Export as PDF"
        aria-label="Export as PDF"
      >
        <Download size={18} />
      </button>
      {onEdit && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onEdit(); }}
          disabled={isDeleting}
          className="entry-modal-action-btn entry-modal-action-btn--primary"
          title="Edit entry"
          aria-label="Edit entry"
        >
          <Pencil size={18} />
        </button>
      )}
      {onDelete && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          disabled={isDeleting}
          className="entry-modal-action-btn entry-modal-action-btn--danger"
          title="Delete entry"
          aria-label="Delete entry"
        >
          {isDeleting ? <Trash2 size={18} opacity={0.5} /> : <Trash2 size={18} />}
        </button>
      )}
    </>
  );

  return (
    <Modal open={isOpen} onClose={onClose} title={modalTitle} headerActions={headerActions} maxWidth={720}>
      {title && (
        <div className="history-markdown entry-modal-heading">
          <h1>{title}</h1>
        </div>
      )}
      {entry.selections?.length > 0 && (
        <div className="tag-list entry-modal-tags">
          {entry.selections.map((s) => (
            <span key={s.id} className="tag">{s.name}</span>
          ))}
        </div>
      )}
      <div className="history-markdown">
        <ReactMarkdown>{body}</ReactMarkdown>
      </div>
    </Modal>
  );
};

export default EntryModal;
