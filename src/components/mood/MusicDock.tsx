import { useState, useEffect, useRef, useCallback } from 'react';
import type { MouseEvent as ReactMouseEvent, TouchEvent as ReactTouchEvent } from 'react';
import { Play, Pause, Square, Disc, Sun, Moon } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext'; // Consuming your app's global Theme context
import useMediaQuery from '../../hooks/useMediaQuery';
import { useI18n } from '../../i18n';
import type { MusicTrack } from '../../types/api';

/**
 * Payload broadcast on the `playMoodMusic` CustomEvent: the server's
 * MusicTrack plus the picked mood's accent color (added by MoodPicker).
 */
export interface PlayMoodMusicDetail extends MusicTrack {
  color: string;
}

declare global {
  interface WindowEventMap {
    playMoodMusic: CustomEvent<PlayMoodMusicDetail>;
  }
}

interface Position {
  x: number;
  y: number;
}

const MusicDock = () => {
  // Theme state integration. useTheme always returns a value (the context is
  // created with a non-null default), so the old `|| {}` fallback and the
  // destructuring defaults were dead branches; theme/cycle are always present.
  const { theme, cycle: toggleTheme } = useTheme();
  const { t } = useI18n();
  const isDarkMode = theme === 'dark';
  // Shares the 640px mobile breakpoint convention (src/index.css) instead of
  // an ad-hoc window.innerWidth check, so JS and CSS agree.
  const isMobile = useMediaQuery('(max-width: 640px)');

  const [track, setTrack] = useState<PlayMoodMusicDetail | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const dockRef = useRef<HTMLDivElement | null>(null);

  // Dragging interaction states
  const [isDragging, setIsDragging] = useState(false);
  const [position, setPosition] = useState<Position>({ x: 0, y: 0 });
  const dragStart = useRef<Position>({ x: 0, y: 0 });

  // The audio element is created imperatively rather than as a JSX <audio>
  // (jsx-a11y/media-has-caption): it is purely functional and never rendered
  // or perceivable, captions are not applicable to instrumental mood tracks,
  // and the dock already surfaces track name/artist as visible text.
  useEffect(() => {
    const audio = new Audio();
    const handleEnded = () => setIsPlaying(false);
    audio.addEventListener('ended', handleEnded);
    audioRef.current = audio;
    return () => {
      audio.removeEventListener('ended', handleEnded);
      audio.pause();
      audioRef.current = null;
    };
  }, []);

  useEffect(() => {
    const handlePlay = (e: CustomEvent<PlayMoodMusicDetail>) => {
      setTrack(e.detail);
      setIsPlaying(true);
      if (audioRef.current) {
        audioRef.current.src = e.detail.audio_url;
        audioRef.current.play().catch(() => setIsPlaying(false));
      }
    };

    window.addEventListener('playMoodMusic', handlePlay);
    return () => window.removeEventListener('playMoodMusic', handlePlay);
  }, []);

  // --- Core Drag Calculation Engine ---
  const startDrag = (clientX: number, clientY: number) => {
    if (!dockRef.current) return;
    setIsDragging(true);

    dragStart.current = {
      x: clientX - position.x,
      y: clientY - position.y
    };
  };

  // useCallback so the global-listener effect below can list it as a
  // dependency without re-subscribing on every position update: it reads
  // position via dragStart ref math, never the position state itself.
  const moveDrag = useCallback((clientX: number, clientY: number) => {
    if (!isDragging || !dockRef.current) return;

    let newX = clientX - dragStart.current.x;
    let newY = clientY - dragStart.current.y;

    // Boundary constraints based on viewport sizing
    const rect = dockRef.current.getBoundingClientRect();
    const windowWidth = window.innerWidth;
    const windowHeight = window.innerHeight;

    const defaultBottom = isMobile ? 90 : 24;
    const defaultRight = isMobile ? 16 : 24;

    const initialRightBoundary = windowWidth - rect.width - defaultRight;
    const initialBottomBoundary = windowHeight - rect.height - defaultBottom;

    // Pin bounding guardrails so it doesn't leave the screen
    if (newX < -initialRightBoundary) newX = -initialRightBoundary;
    if (newX > defaultRight) newX = defaultRight;
    if (newY < -initialBottomBoundary) newY = -initialBottomBoundary;
    if (newY > defaultBottom) newY = defaultBottom;

    setPosition({ x: newX, y: newY });
  }, [isDragging, isMobile]);

  const endDrag = useCallback(() => {
    setIsDragging(false);
  }, []);

  // --- Desktop Mouse Draggable Triggers ---
  const handleMouseDown = (e: ReactMouseEvent<HTMLDivElement>) => {
    if (e.button !== 0) return; // Only allow left-clicks to drag
    startDrag(e.clientX, e.clientY);
  };

  // Global mouse event tracking for seamless dragging across screen. The
  // mousemove handler lives inside the effect (DOM MouseEvent, not React's)
  // so the dependency list is exact; moveDrag/endDrag are stable useCallbacks.
  useEffect(() => {
    if (!isDragging) return undefined;
    const handleMouseMove = (e: MouseEvent) => {
      moveDrag(e.clientX, e.clientY);
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', endDrag);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', endDrag);
    };
  }, [isDragging, moveDrag, endDrag]);

  // --- Mobile Touch Draggable Triggers ---
  const handleTouchStart = (e: ReactTouchEvent<HTMLDivElement>) => {
    const touch = e.touches[0];
    if (!touch) return;
    startDrag(touch.clientX, touch.clientY);
  };

  const handleTouchMove = (e: ReactTouchEvent<HTMLDivElement>) => {
    const touch = e.touches[0];
    if (!touch) return;
    moveDrag(touch.clientX, touch.clientY);
  };

  const togglePlay = () => {
    if (!audioRef.current || !track) return;
    if (isPlaying) {
      audioRef.current.pause();
    } else {
      audioRef.current.play();
    }
    setIsPlaying(!isPlaying);
  };

  const stopMusic = () => {
    if (audioRef.current) {
      audioRef.current.pause();
      audioRef.current.src = "";
    }
    setTrack(null);
    setIsPlaying(false);
    setPosition({ x: 0, y: 0 }); // Reset positioning alignment
  };

  // Border color falls back to matching neutral tokens if no track vibe color is present
  const dockBorderColor = track ? track.color : (isDarkMode ? '#333' : '#e2e8f0');
  const dockShadowColor = track ? `${track.color}66` : (isDarkMode ? 'rgba(0,0,0,0.5)' : 'rgba(148,163,184,0.15)');

  return (
    <div
      ref={dockRef}
      className={`music-dock ${isDarkMode ? 'music-dock--dark' : 'music-dock--light'}`}
      // The drag handlers are a pointer-only positioning enhancement (the
      // dock's position is purely cosmetic; every real control inside is a
      // keyboard-accessible <button>), so the wrapper is declared
      // presentational rather than pretending to be a widget.
      role="presentation"
      onMouseDown={handleMouseDown}
      onTouchStart={handleTouchStart}
      onTouchMove={handleTouchMove}
      onTouchEnd={endDrag}
      style={{
        border: `1px solid ${dockBorderColor}`,
        boxShadow: track ? `0 8px 32px -8px ${dockShadowColor}` : `0 8px 32px ${dockShadowColor}`,
        transform: `translate(${position.x}px, ${position.y}px)`,
        touchAction: 'none', /* Blocks mobile viewport scrolling layouts during drag sweeps */
        cursor: isDragging ? 'grabbing' : 'grab',
        transition: isDragging ? 'none' : 'transform 0.2s ease-out, border-color 0.3s ease, box-shadow 0.3s ease, background-color 0.3s ease'
      }}
    >
      {/* Spinning Disc */}
      <div
        className="music-dock__disc"
        style={{
          color: track ? track.color : (isDarkMode ? '#444' : '#cbd5e1'),
          animation: isPlaying ? 'spin 3s linear infinite' : 'none',
        }}
      >
        <Disc size={32} />
      </div>

      {/* Info Section */}
      <div className="music-dock__info" style={{ userSelect: 'none' }}>
        <div className="music-dock__track-name" style={{ color: track ? (isDarkMode ? 'white' : '#0f172a') : (isDarkMode ? '#666' : '#94a3b8') }}>
          {track ? track.track_name : t('moods.noVibeDetected')}
        </div>
        <div className="music-dock__artist" style={{ color: isDarkMode ? '#888' : '#64748b' }}>
          {track ? track.artist : t('moods.selectMood')}
        </div>
      </div>

      {/* Controls & Theme Toggle */}
      <div
        className="music-dock__controls"
        // stopPropagation-only handlers (keep button clicks from starting a
        // drag) are an implementation detail, not an interaction — the
        // buttons inside carry the real, keyboard-accessible semantics.
        role="presentation"
        onMouseDown={(e) => e.stopPropagation()}
        onTouchStart={(e) => e.stopPropagation()}
      >
        {track && (
          <>
            <button onClick={togglePlay} className="music-dock__btn music-dock__btn--action">
              {isPlaying ? <Pause size={20} /> : <Play size={20} />}
            </button>
            <button onClick={stopMusic} className="music-dock__btn music-dock__btn--stop">
              <Square size={18} fill="#ff4d4d" />
            </button>
          </>
        )}

        {/* Night / Day Mode Toggle Button */}
        {toggleTheme && (
          <button
            onClick={toggleTheme}
            className="music-dock__btn music-dock__btn--theme"
            aria-label={t('common.toggleLayoutThemeAria')}
          >
            {isDarkMode ? <Sun size={18} /> : <Moon size={18} />}
          </button>
        )}
      </div>

      <style>{`
        /* Desktop Base layout dynamic container specifications */
        .music-dock {
          position: fixed;
          bottom: calc(24px + env(safe-area-inset-bottom));
          right: calc(24px + env(safe-area-inset-right));
          width: 320px; /* Slightly widened to gracefully host the toggle control button */
          backdrop-filter: blur(12px);
          border-radius: 20px;
          padding: 16px;
          /* Below Modal and Toast (var(--z-modal) / var(--z-toast)) so
             dialogs/toasts always win. See src/index.css for the full
             z-index scale. */
          z-index: var(--z-music-dock);
          display: flex;
          align-items: center;
          gap: 12px;
        }

        /* Dark Mode Color Token Definitions */
        .music-dock--dark {
          background-color: rgba(15, 15, 15, 0.9);
        }
        .music-dock--dark .music-dock__btn--action { color: white; }
        .music-dock--dark .music-dock__btn--theme { color: #facc15; }

        /* Light Mode Color Token Definitions */
        .music-dock--light {
          background-color: rgba(255, 255, 255, 0.9);
        }
        .music-dock--light .music-dock__btn--action { color: #0f172a; }
        .music-dock--light .music-dock__btn--theme { color: #475569; }

        .music-dock__disc {
          display: flex;
          align-items: center;
        }

        .music-dock__info {
          flex: 1;
          overflow: hidden;
        }

        .music-dock__track-name {
          font-weight: bold;
          font-size: 13px;
          white-space: nowrap;
          overflow: hidden;
          text-overflow: ellipsis;
        }

        .music-dock__artist {
          font-size: 12px;
          white-space: nowrap;
          overflow: hidden;
          text-overflow: ellipsis;
        }

        .music-dock__controls {
          display: flex;
          gap: 8px;
          align-items: center;
          min-height: 28px;
        }

        .music-dock__btn {
          background: none;
          border: none;
          cursor: pointer;
          padding: 4px;
          display: inline-flex;
          align-items: center;
          transition: transform 0.2s ease;
        }

        .music-dock__btn:hover {
          transform: scale(1.1);
        }

        .music-dock__btn--stop { color: #ff4d4d; }

        @keyframes spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }

        /* Mobile Portrait Adaptations — matches the 640px mobile breakpoint
           convention documented in src/index.css */
        @media (max-width: 640px) {
          .music-dock {
            width: 245px; /* Scaled fractionally to secure layout container bounds with the toggle */
            padding: 10px 12px;
            bottom: calc(90px + env(safe-area-inset-bottom));
            right: calc(16px + env(safe-area-inset-right));
            border-radius: 16px;
            gap: 10px;
          }

          .music-dock__info {
            display: block;
          }

          .music-dock__track-name {
            font-size: 12px;
          }

          .music-dock__artist {
            font-size: 12px;
          }

          .music-dock__disc svg {
            width: 24px;
            height: 24px;
          }

          .music-dock__controls {
            gap: 4px;
          }
        }
      `}</style>
    </div>
  );
};

export default MusicDock;
