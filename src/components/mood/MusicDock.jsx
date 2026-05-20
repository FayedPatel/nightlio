import React, { useState, useEffect, useRef } from 'react';
import { Play, Pause, Square, Disc, Sun, Moon } from 'lucide-react';
import { useTheme } from '../../contexts/ThemeContext'; // Consuming your app's global Theme context

const MusicDock = () => {
  // Theme state integration
  const { theme, toggleTheme } = useTheme() || { theme: 'dark', toggleTheme: null }; 
  const isDarkMode = theme === 'dark';

  const [track, setTrack] = useState(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const audioRef = useRef(null);
  const dockRef = useRef(null);

  // Dragging interaction states
  const [isDragging, setIsDragging] = useState(false);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  const dragStart = useRef({ x: 0, y: 0 });

  useEffect(() => {
    const handlePlay = (e) => {
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
  const startDrag = (clientX, clientY) => {
    if (!dockRef.current) return;
    setIsDragging(true);
    
    dragStart.current = {
      x: clientX - position.x,
      y: clientY - position.y
    };
  };

  const moveDrag = (clientX, clientY) => {
    if (!isDragging || !dockRef.current) return;
    
    let newX = clientX - dragStart.current.x;
    let newY = clientY - dragStart.current.y;

    // Boundary constraints based on viewport sizing
    const rect = dockRef.current.getBoundingClientRect();
    const windowWidth = window.innerWidth;
    const windowHeight = window.innerHeight;

    const isMobile = windowWidth <= 640;
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
  };

  const endDrag = () => {
    setIsDragging(false);
  };

  // --- Desktop Mouse Draggable Triggers ---
  const handleMouseDown = (e) => {
    if (e.button !== 0) return; // Only allow left-clicks to drag
    startDrag(e.clientX, e.clientY);
  };

  const handleMouseMove = (e) => {
    moveDrag(e.clientX, e.clientY);
  };

  // Global mouse event tracking for seamless dragging across screen
  useEffect(() => {
    if (isDragging) {
      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', endDrag);
    }
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', endDrag);
    };
  }, [isDragging, position]);

  // --- Mobile Touch Draggable Triggers ---
  const handleTouchStart = (e) => {
    const touch = e.touches[0];
    startDrag(touch.clientX, touch.clientY);
  };

  const handleTouchMove = (e) => {
    const touch = e.touches[0];
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
          {track ? track.track_name : "No vibe detected"}
        </div>
        <div className="music-dock__artist" style={{ color: isDarkMode ? '#888' : '#64748b' }}>
          {track ? track.artist : "Select a mood"}
        </div>
      </div>

      {/* Controls & Theme Toggle */}
      <div 
        className="music-dock__controls" 
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
            aria-label="Toggle layout theme"
          >
            {isDarkMode ? <Sun size={18} /> : <Moon size={18} />}
          </button>
        )}
      </div>

      <audio ref={audioRef} onEnded={() => setIsPlaying(false)} />
      
      <style>{`
        /* Desktop Base layout dynamic container specifications */
        .music-dock {
          position: fixed;
          bottom: 24px;
          right: 24px;
          width: 320px; /* Slightly widened to gracefully host the toggle control button */
          backdrop-filter: blur(12px);
          border-radius: 20px;
          padding: 16px;
          z-index: 99999;
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
          font-size: 11px;
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

        /* Mobile Portrait Adaptations */
        @media (max-width: 640px) {
          .music-dock {
            width: 245px; /* Scaled fractionally to secure layout container bounds with the toggle */
            padding: 10px 12px;
            bottom: 90px;
            right: 16px;
            border-radius: 16px;
            gap: 10px;
          }

          .music-dock__info {
            display: block;
          }

          .music-dock__track-name {
            font-size: 11px;
          }

          .music-dock__artist {
            font-size: 10px;
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