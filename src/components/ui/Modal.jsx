import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import useMediaQuery from '../../hooks/useMediaQuery';

// Below this the sheet snaps closed instead of springing back, matched
// against whichever is smaller: a fixed distance or a third of the panel.
const CLOSE_DRAG_PX = 120;

const Modal = ({ open, title, headerActions, children, onClose, maxWidth = 520 }) => {
  // Shares the 640px mobile breakpoint convention (src/index.css) with the
  // rest of the app instead of an ad-hoc width check.
  const isMobile = useMediaQuery('(max-width: 640px)');
  const panelRef = useRef(null);
  const dragRef = useRef({ active: false, startY: 0, baseY: 0, pointerId: null });
  const [dragY, setDragY] = useState(0);
  const [isDragging, setIsDragging] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        if (typeof onClose === 'function') onClose();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, onClose]);

  // Reset any leftover drag offset whenever the sheet (re)opens so a
  // previous partial swipe doesn't leave it rendered half off-screen.
  useEffect(() => {
    if (open) setDragY(0);
  }, [open]);

  if (!open) return null;

  const handleHandlePointerDown = (e) => {
    if (!isMobile) return;
    dragRef.current = { active: true, startY: e.clientY, baseY: dragY, pointerId: e.pointerId };
    setIsDragging(true);
    if (e.currentTarget.setPointerCapture) e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handleHandlePointerMove = (e) => {
    const d = dragRef.current;
    if (!d.active || e.pointerId !== d.pointerId) return;
    const next = d.baseY + (e.clientY - d.startY);
    setDragY(next < 0 ? 0 : next); // only allow dragging the sheet down, never above its resting position
  };

  const handleHandlePointerUp = (e) => {
    const d = dragRef.current;
    if (!d.active || e.pointerId !== d.pointerId) return;
    dragRef.current.active = false;
    setIsDragging(false);
    const panelHeight = panelRef.current ? panelRef.current.offsetHeight : 0;
    const threshold = Math.min(CLOSE_DRAG_PX, panelHeight * 0.3 || CLOSE_DRAG_PX);
    if (dragY > threshold) {
      if (typeof onClose === 'function') onClose();
    }
    setDragY(0);
  };

  const modal = (
    <div className="ui-modal-overlay" aria-modal="true" role="dialog">
      <div className="ui-modal-backdrop" onClick={onClose} />
      <div
        ref={panelRef}
        className={`ui-modal-panel${isMobile ? ' ui-modal-panel--sheet' : ''}`}
        style={
          isMobile
            ? {
                transform: `translateY(${dragY}px)`,
                transition: isDragging ? 'none' : 'transform 220ms cubic-bezier(.2,.7,.3,1)',
              }
            : { maxWidth }
        }
      >
        {isMobile && (
          <div
            className="ui-modal-handle"
            onPointerDown={handleHandlePointerDown}
            onPointerMove={handleHandlePointerMove}
            onPointerUp={handleHandlePointerUp}
            onPointerCancel={handleHandlePointerUp}
          >
            <span className="ui-modal-handle__bar" />
          </div>
        )}
        <div className="ui-modal-header">
          <div className="ui-modal-header-row">
            <h3 className="ui-modal-title">{title}</h3>
            {headerActions && (
              <div className="ui-modal-header-actions">{headerActions}</div>
            )}
          </div>
        </div>
        <div className="ui-modal-body">{children}</div>
      </div>
      <style>{`
        .ui-modal-overlay {
          position: fixed;
          inset: 0;
          z-index: var(--z-modal);
        }
        .ui-modal-backdrop {
          position: absolute;
          inset: 0;
          background: var(--overlay);
        }
        .ui-modal-panel {
          position: absolute;
          left: 50%;
          top: 50%;
          transform: translate(-50%, -50%);
          background: var(--bg-card);
          color: var(--fg-body);
          width: calc(100% - 32px);
          /* Sheet mode sets width: 100% below — without border-box that's
             100% of the overlay plus this 1px border on each side, a couple
             of px of horizontal overflow on exactly the viewport-edge-to-
             edge mobile sheet where it's most visible. */
          box-sizing: border-box;
          border-radius: 16px;
          box-shadow: var(--shadow-3);
          border: 1px solid var(--border);
          overscroll-behavior: contain;
        }
        .ui-modal-header {
          padding: 16px 20px;
          border-bottom: 1px solid var(--border);
        }
        .ui-modal-header-row {
          display: flex;
          align-items: center;
          justify-content: space-between;
          gap: 12px;
        }
        .ui-modal-title {
          margin: 0;
          font-size: 1.1rem;
          min-width: 0;
        }
        .ui-modal-header-actions {
          display: flex;
          align-items: center;
          gap: 8px;
          flex-shrink: 0;
        }
        .ui-modal-body {
          padding: 20px;
        }

        /* Mobile: bottom sheet instead of a centered dialog. dvh (not vh) so
           the sheet doesn't overshoot when the mobile URL bar collapses. */
        @media (max-width: 640px) {
          .ui-modal-panel--sheet {
            position: fixed;
            left: 0;
            right: 0;
            bottom: 0;
            top: auto;
            transform: none; /* JS drives translateY via inline style while dragging */
            width: 100%;
            max-width: 100%;
            max-height: 90dvh;
            display: flex;
            flex-direction: column;
            border-radius: 20px 20px 0 0;
            padding-bottom: env(safe-area-inset-bottom);
            animation: ui-modal-slide-up 220ms cubic-bezier(.2,.7,.3,1);
          }
          .ui-modal-panel--sheet .ui-modal-header {
            flex-shrink: 0;
          }
          .ui-modal-panel--sheet .ui-modal-body {
            flex: 1;
            min-height: 0;
            overflow-y: auto;
            overscroll-behavior: contain;
          }
          @keyframes ui-modal-slide-up {
            from { transform: translateY(100%); }
            to { transform: translateY(0); }
          }
          .ui-modal-handle {
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 44px;
            touch-action: none;
            cursor: grab;
            flex-shrink: 0;
          }
          .ui-modal-handle:active {
            cursor: grabbing;
          }
          .ui-modal-handle__bar {
            width: 40px;
            height: 4px;
            border-radius: 999px;
            background: color-mix(in oklab, var(--fg-body), transparent 65%);
          }
        }
      `}</style>
    </div>
  );

  // Portal straight to document.body (Phase 9d fix): rendered in place,
  // this "fixed" overlay was silently broken whenever an ancestor set
  // `backdrop-filter` — .app-main does, for its glass-panel look — because
  // a non-none backdrop-filter establishes a new containing block for
  // position:fixed descendants, same as `transform`. The symptom: the
  // panel's `bottom: 0` / centered position resolved against app-main's
  // full (scrollable, often 2000px+) content box instead of the viewport,
  // so the sheet/dialog rendered far off-screen instead of pinned in
  // view. Escaping to document.body sidesteps every such ancestor.
  if (typeof document === 'undefined') return modal;
  return createPortal(modal, document.body);
};

export default Modal;
