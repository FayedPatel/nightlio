import { useState, useEffect } from 'react';
import { Search, X } from 'lucide-react';
import './SearchBar.css';

export default function SearchBar({
  entries = [],
  onSearch,
  placeholder = "Search entries...",
  searchFields = ['content', 'date']  // Which fields to search
}) {
  const [query, setQuery] = useState('');

  // Real-time search as user types; results are delivered to the parent via
  // onSearch rather than rendered here (the inline dropdown was removed).
  useEffect(() => {
    if (!query.trim()) {
      if (onSearch) onSearch(null); // Return null when search is cleared
      return;
    }

    const queryLower = query.toLowerCase();

    // Filter entries by search query
    const filtered = entries.filter(entry => {
      // Search in specified fields
      return searchFields.some(field => {
        const value = entry[field];
        if (!value) return false;
        return String(value).toLowerCase().includes(queryLower);
      });
    });

    // Callback to parent component
    if (onSearch) {
      onSearch(filtered);
    }
  }, [query, entries, searchFields, onSearch]);

  return (
    <div className="search-bar-container">
      <div className="search-bar-input-wrapper">
        <Search size={18} className="search-bar-icon" />
        <input
          type="text"
          placeholder={placeholder}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="search-bar-input"
        />
        {query && (
          <button
            className="search-bar-clear"
            onClick={() => setQuery('')}
          >
            <X size={18} />
          </button>
        )}
      </div>
    </div>
  );
}
