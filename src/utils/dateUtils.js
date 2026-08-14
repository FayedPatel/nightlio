// Entry-date helpers. mood_entries.date is free-form TEXT on the backend and
// two shapes coexist in the wild: US locale strings (M/D/YYYY, what the
// frontend's toLocaleDateString() used to send) and ISO (YYYY-MM-DD, what new
// entries send now). Anything that compares or displays entry dates must go
// through these helpers so both shapes keep working.

const pad2 = (value) => String(value).padStart(2, '0');

// Local-time ISO day key (YYYY-MM-DD). Never uses toISOString(), which would
// shift days across timezones.
export const toISODateKey = (date) =>
  `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;

export const todayISO = () => toISODateKey(new Date());

export const yesterdayISO = () => {
  const date = new Date();
  date.setDate(date.getDate() - 1);
  return toISODateKey(date);
};

// Normalise a stored entry.date (ISO or M/D/YYYY) to an ISO key. String
// splitting on purpose: new Date('YYYY-MM-DD') parses as UTC midnight and can
// land on the wrong local day.
export const entryDateKey = (dateStr) => {
  if (typeof dateStr !== 'string') return '';
  const iso = dateStr.match(/^(\d{4})-(\d{1,2})-(\d{1,2})$/);
  if (iso) return `${iso[1]}-${pad2(iso[2])}-${pad2(iso[3])}`;
  const usLocale = dateStr.match(/^(\d{1,2})\/(\d{1,2})\/(\d{4})$/);
  if (usLocale) return `${usLocale[3]}-${pad2(usLocale[1])}-${pad2(usLocale[2])}`;
  return dateStr;
};

// Human display for a stored entry.date — ISO rows would otherwise render raw
// as "2026-08-13". Unparseable values fall back to the raw string.
export const formatEntryDate = (dateStr) => {
  const parts = entryDateKey(dateStr).match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!parts) return dateStr ?? '';
  const date = new Date(Number(parts[1]), Number(parts[2]) - 1, Number(parts[3]));
  return date.toLocaleDateString();
};
