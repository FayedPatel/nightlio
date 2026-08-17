/**
 * Nightlio API contract types.
 *
 * Derived from contract/openapi.yaml (the merged OpenAPI 3.1 document), which
 * is itself derived from golden fixtures recorded against the live Flask API
 * (contract/fixtures/**). Tailored to the endpoints the frontend actually
 * consumes via src/services/api.js and src/services/statsApi.js.
 *
 * Type-only module: no runtime code, no side effects. Strict-mode clean.
 *
 * Contract rules reflected here:
 * - Every error body is `{ error: string }` (ApiError). Compare status codes
 *   and shapes, never the message text.
 * - Six endpoints return BARE arrays (MoodEntry[], Goal[], Group[],
 *   Achievement[], EntrySelection[], GoalCompletion[]); /api/activity is the
 *   only paginated endpoint (ActivityPage).
 * - `mood_distribution` has sparse STRING keys ("1".."5") — never a fixed map.
 * - Statistics nulls are real: empty accounts have null lowest/highest mood
 *   and first/last entry dates, and `average_mood` is the integer 0 (falsy
 *   average) or a rounded float otherwise — both are `number` here.
 * - Timestamps are SQLite CURRENT_TIMESTAMP text: UTC "YYYY-MM-DD HH:MM:SS",
 *   no timezone suffix.
 */

// ---------------------------------------------------------------------------
// Shared primitives
// ---------------------------------------------------------------------------

/** Universal error envelope. Message text is NOT part of the graded contract. */
export interface ApiError {
  error: string;
}

/** Valid mood values. Requests and stored entries are always 1..5. */
export type MoodValue = 1 | 2 | 3 | 4 | 5;

/**
 * SQLite CURRENT_TIMESTAMP text: "YYYY-MM-DD HH:MM:SS" (UTC, space separator,
 * no timezone suffix). Alias for documentation only.
 */
export type SqliteTimestamp = string;

/** ISO "YYYY-MM-DD". Mood-entry dates may also be legacy US "M/D/YYYY". */
export type IsoDate = string;

// ---------------------------------------------------------------------------
// Config (GET /api/config)
// ---------------------------------------------------------------------------

export interface AppConfig {
  enable_oidc: boolean;
  enable_mood_music: boolean;
  enable_local_login: boolean;
  /** Absolute http(s) URL only when OIDC is enabled; null otherwise. */
  signup_url: string | null;
}

// ---------------------------------------------------------------------------
// Auth (POST /api/auth/local/login, /verify, /logout, /local/register)
// ---------------------------------------------------------------------------

/**
 * The four-key user projection returned by every auth endpoint — never the
 * full DB row. name/email are non-null in practice for local/self-host users
 * (the DB layer fills defaults) but nullable in the schema; OIDC users may
 * genuinely carry nulls. avatar_url is null except via OIDC.
 */
export interface User {
  /** Autoincrement id; gaps are normal (upsert burn). Never compare exact ids. */
  id: number;
  name: string | null;
  email: string | null;
  avatar_url: string | null;
}

export interface LoginResponse {
  /** HS256 JWT; the same token is also set as the nightlio_token cookie. */
  token: string;
  user: User;
}

/**
 * Body for POST /api/auth/local/login. The whole body is optional: omitting
 * it (or sending no keys) triggers the credential-free self-host branch.
 * Mere presence of either key switches to the credentialed branch.
 */
export interface LocalLoginRequest {
  username?: string;
  password?: string;
}

export interface RegisterRequest {
  username: string;
  /** Minimum 8 characters. */
  password: string;
  /** Defaults server-side to "<username>@localhost". */
  email?: string;
  /** Defaults server-side to the username. */
  name?: string;
}

/** 201 body. Registering does NOT log the new user in — no token, no cookie. */
export interface RegisterResponse {
  status: 'success';
  user: User;
}

/** POST /api/auth/verify 200 body. */
export interface VerifyTokenResponse {
  user: User;
}

/** POST /api/auth/logout 200 body (idempotent; clearing Set-Cookie attached). */
export interface LogoutResponse {
  status: 'success';
}

// ---------------------------------------------------------------------------
// Mood entries
// ---------------------------------------------------------------------------

export interface MoodEntry {
  id: number;
  /** Echoed verbatim as stored: "YYYY-MM-DD" or legacy "M/D/YYYY". */
  date: string;
  mood: MoodValue;
  content: string;
  /**
   * Normally a SqliteTimestamp, but a client-supplied `time` (backdating)
   * overwrites the column verbatim.
   */
  created_at: string;
  /** Never null — defaults to CURRENT_TIMESTAMP at insert. */
  updated_at: SqliteTimestamp;
}

export interface CreateMoodEntryRequest {
  /** 1..5; a falsy 0 fails the presence check ("Missing required fields"). */
  mood: MoodValue;
  /** "YYYY-MM-DD" or "M/D/YYYY"; stored verbatim; max 1 day in the future. */
  date: string;
  /** Must not be empty/whitespace. */
  content: string;
  /** Optional backdating: truthy value is written into created_at verbatim. */
  time?: string | null;
  /** Group option ids to attach; null is treated as []. */
  selected_options?: number[] | null;
}

export type AchievementType =
  | 'first_entry'
  | 'week_warrior'
  | 'consistency_king'
  | 'data_lover'
  | 'mood_master';

export type AchievementRarity = 'common' | 'uncommon' | 'rare' | 'legendary';

/** POST /api/mood 201 body. */
export interface CreateMoodEntryResponse {
  status: 'success';
  entry_id: number;
  /** Same NewAchievement metadata objects as POST /api/achievements/check. */
  new_achievements: NewAchievement[];
  message: string;
}

/** PUT /api/mood/{id} body — at least one field must be present, else 400. */
export interface UpdateMoodEntryRequest {
  mood?: MoodValue;
  content?: string;
  date?: string;
  /** Truthy value overwrites created_at (backdating). */
  time?: string | null;
  /** REPLACES all selections; null clears them (treated as []). */
  selected_options?: number[] | null;
}

/**
 * Unlike GET /api/mood/{id} (plain MoodEntry, no selections key), the PUT
 * response embeds the entry WITH its selections array.
 */
export interface UpdateMoodEntryResponse {
  status: 'success';
  message: string;
  entry: MoodEntry & { selections: EntrySelection[] };
}

/** DELETE responses are always 200 + JSON (never 204). */
export interface DeleteResponse {
  status: 'success';
  message: string;
}

/**
 * Row of GET /api/mood/{entry_id}/selections (bare array) and of the
 * selections embedded in UpdateMoodEntryResponse. group_id is NOT included.
 * A nonexistent/foreign entry returns 200 [] — never 404.
 */
export interface EntrySelection {
  /** group_options id. */
  id: number;
  /** Option name. */
  name: string;
  group_name: string;
}

// ---------------------------------------------------------------------------
// Statistics (GET /api/statistics, /extended, /heatmap, /digest, /streak)
// ---------------------------------------------------------------------------

/**
 * Sparse distribution: keys are the STRING form of logged mood values only —
 * never a fixed 1..5 map; {} for an empty account.
 */
export type MoodDistribution = Partial<Record<'1' | '2' | '3' | '4' | '5', number>>;

export interface CoreStatistics {
  total_entries: number;
  /**
   * round(AVG(mood), 2): a float when entries exist, the INTEGER 0 for an
   * empty account. Both are `number` in JS.
   */
  average_mood: number;
  lowest_mood: number | null;
  highest_mood: number | null;
  /**
   * Lexicographic MIN over RAW stored date strings — with mixed formats a
   * US "M/D/YYYY" date sorts after every ISO date regardless of chronology.
   */
  first_entry_date: string | null;
  /** Lexicographic MAX over raw strings (same caveat). */
  last_entry_date: string | null;
}

/**
 * GET /api/statistics 200 body. As of the rewrite this read no longer increments
 * stats_views — that moved to the explicit POST /api/statistics/view.
 */
export interface Statistics {
  statistics: CoreStatistics;
  mood_distribution: MoodDistribution;
  current_streak: number;
}

/**
 * POST /api/statistics/view 200 body. counted=false when today's view was
 * already recorded (at most one stats_views increment per calendar day).
 */
export interface RecordStatisticsViewResponse {
  counted: boolean;
}

export interface RollingAveragePoint {
  /** Normalized ISO "YYYY-MM-DD". */
  date: IsoDate;
  average_mood: number;
  entry_count: number;
  /** Trailing mean over up to 7 logged days (ROWS BETWEEN, not calendar days). */
  rolling_7: number;
  rolling_30: number;
}

export type WeekdayName =
  | 'Sunday'
  | 'Monday'
  | 'Tuesday'
  | 'Wednesday'
  | 'Thursday'
  | 'Friday'
  | 'Saturday';

export interface WeekdayAverage {
  /** 0=Sunday .. 6=Saturday; always exactly 7 rows. */
  weekday: 0 | 1 | 2 | 3 | 4 | 5 | 6;
  name: WeekdayName;
  average_mood: number | null;
  entry_count: number;
}

export interface MoodVolatility {
  window_days: 30;
  /** Entries in the trailing 30 calendar days from server-LOCAL now. */
  entry_count: number;
  /** Null when entry_count is 0. */
  average_mood: number | null;
  /** Sample stddev; null when entry_count < 2. */
  stddev: number | null;
}

export interface TagCorrelation {
  option_id: number;
  option_name: string;
  group_id: number;
  group_name: string;
  average_mood_selected: number | null;
  entry_count_selected: number;
  average_mood_not_selected: number | null;
  entry_count_not_selected: number;
}

export interface GoalCorrelation {
  goal_id: number;
  goal_name: string;
  average_mood_completed: number | null;
  entry_count_completed: number;
  average_mood_not_completed: number | null;
  entry_count_not_completed: number;
}

export interface DigestTag {
  option_id: number;
  option_name: string;
  group_name: string;
  times_selected: number;
}

/** GET /api/statistics/digest 200 body (and monthly_digest in ExtendedStats). */
export interface Digest {
  year: number;
  month: number;
  entries_logged: number;
  /** Null when the month has no entries. */
  average_mood: number | null;
  previous_average_mood: number | null;
  /** average_mood - previous_average_mood; null when either month is empty. */
  mood_trend: number | null;
  /** At most 5, by times_selected DESC then option name ASC. */
  top_tags: DigestTag[];
  longest_streak: number;
}

/** GET /api/statistics/extended 200 body. */
export interface ExtendedStats {
  rolling_averages: {
    series: RollingAveragePoint[];
    /** series.length. */
    count: number;
  };
  weekday_averages: WeekdayAverage[];
  mood_volatility: MoodVolatility;
  /** [] when the user has no entries. */
  tag_correlations: TagCorrelation[];
  /** [] when the user has no mood entries. */
  goal_correlations: GoalCorrelation[];
  /** Always the CURRENT server-local year/month. */
  monthly_digest: Digest;
}

export interface HeatmapDay {
  /** Normalized ISO "YYYY-MM-DD". */
  date: IsoDate;
  average_mood: number;
  entry_count: number;
}

/** GET /api/statistics/heatmap 200 body. Only logged days appear. */
export interface Heatmap {
  year: number;
  days: HeatmapDay[];
  /** days.length. */
  days_logged: number;
}

/** GET /api/streak 200 body. */
export interface Streak {
  current_streak: number;
  /** "Current streak: {n} day(s)" — pluralized, including "0 days". */
  message: string;
}

// ---------------------------------------------------------------------------
// Activity feed (GET /api/activity — the only paginated endpoint)
// ---------------------------------------------------------------------------

export interface ActivityEvent {
  id: number;
  user_id: number;
  /**
   * Known values: login, entry_created, entry_edited, entry_deleted,
   * achievement_unlocked, goal_completed. Open set — treat as string.
   */
  event_type: string;
  /** Parsed JSON blob; null when absent or malformed. Shape varies by event_type. */
  metadata: Record<string, unknown> | null;
  created_at: SqliteTimestamp;
}

export interface ActivityPage {
  activities: ActivityEvent[];
  /**
   * Last row's id when older rows exist beyond this page, else null —
   * including on a full FINAL page (contract change: the server peeks
   * limit + 1 rows, so a null cursor always means the walk is done and no
   * follow-up request is needed).
   */
  next_cursor: number | null;
}

// ---------------------------------------------------------------------------
// Preferences (GET/PUT /api/preferences)
// ---------------------------------------------------------------------------

/** Values accepted by PUT /api/preferences. */
export type ThemeName = 'default' | 'light' | 'dark' | 'synthwave';

/**
 * GET body. theme is null for a user who has never PUT a theme (the frontend
 * treats null as "default"). Reads echo whatever string is stored — the enum
 * is enforced only on writes, so reads cannot be a closed enum.
 */
export interface Preferences {
  theme: string | null;
}

export interface UpdatePreferencesRequest {
  theme: ThemeName;
}

export interface UpdatePreferencesResponse {
  status: 'success';
  theme: ThemeName;
}

// ---------------------------------------------------------------------------
// Groups (GET/POST /api/groups, POST /api/groups/{id}/options, DELETEs)
// ---------------------------------------------------------------------------

/** Exactly two keys; the parent group_id is NOT included. */
export interface GroupOption {
  id: number;
  name: string;
}

/** Exactly three keys — no user_id, created_at, or counts. */
export interface Group {
  id: number;
  name: string;
  /** Ordered by option name; [] when the group has no options. */
  options: GroupOption[];
}

export interface CreateGroupRequest {
  /** Stored trimmed; empty or whitespace-only -> 400. */
  name: string;
}

export interface CreateGroupResponse {
  status: 'success';
  group_id: number;
  message: string;
}

export interface CreateGroupOptionRequest {
  /** Stored trimmed; empty or whitespace-only -> 400. */
  name: string;
}

/**
 * NOTE: an unknown or foreign group_id is a 400 ("Group not found for
 * user"), NOT a 404.
 */
export interface CreateGroupOptionResponse {
  status: 'success';
  option_id: number;
  message: string;
}

// ---------------------------------------------------------------------------
// Goals
// ---------------------------------------------------------------------------

/**
 * Goals table row after weekly rollover plus the computed
 * already_completed_today flag. Reads are NOT pure: list/get persist rollover
 * mutations (completed reset, streak recompute, updated_at bump).
 */
export interface Goal {
  id: number;
  user_id: number;
  title: string;
  /** API writes a trimmed string, but legacy rows can surface null. */
  description: string | null;
  frequency_per_week: number;
  /** Completions in the current week's period; clamped to frequency_per_week. */
  completed: number;
  /** Consecutive fully-completed weeks; recomputed only at rollover. */
  streak: number;
  /** ISO Monday of the current week; always non-null on responses (rollover fills it). */
  period_start: IsoDate;
  /** Null until the first logged completion. */
  last_completed_date: IsoDate | null;
  created_at: SqliteTimestamp;
  updated_at: SqliteTimestamp;
  /** Computed per request: last_completed_date == server-local today. */
  already_completed_today: boolean;
}

export interface CreateGoalRequest {
  /** Trimmed server-side; blank -> 400. */
  title: string;
  description?: string;
  /** 1..7. */
  frequency_per_week?: number;
  /** Legacy alias, consulted only when frequency_per_week is absent/falsy. */
  frequency?: number;
}

/** POST /api/goals 201 body — only the new id. */
export interface CreateGoalResponse {
  id: number;
}

/**
 * PUT/PATCH /api/goals/{id} body. All fields optional but at least one must
 * be present — since the contract change an empty object yields 400
 * ("No fields to update") and only a missing goal id yields 404. A blank
 * title is rejected with the same 400 create uses ("Title is required").
 */
export interface UpdateGoalRequest {
  title?: string;
  description?: string;
  /** Lowering it clamps completed to the new value. */
  frequency_per_week?: number;
  /** Legacy alias used only when frequency_per_week is absent. */
  frequency?: number;
}

/** Goal update/delete success body — note the const is "ok", not "success". */
export interface GoalStatusOkResponse {
  status: 'ok';
}

export interface IncrementGoalProgressRequest {
  /** "YYYY-MM-DD" or "M/D/YYYY", normalized to ISO; max 1 day in the future. */
  date?: string;
}

/** POST /api/goals/{id}/progress 200 body: full post-update goal + metadata. */
export interface GoalProgressResponse extends Goal {
  /** true when the (goal, day) pair already existed; counter did not move. */
  already_logged: boolean;
  /** The ISO date this call targeted (request date normalized, or today). */
  logged_date: IsoDate;
}

/**
 * Row of GET /api/goals/{id}/completions (bare array, ascending). A
 * nonexistent/foreign goal id returns 200 [] — never 404.
 */
export interface GoalCompletion {
  date: IsoDate;
}

// ---------------------------------------------------------------------------
// Achievements
// ---------------------------------------------------------------------------

/**
 * Row of GET /api/achievements (bare array, earned_at DESC): the SQLite
 * achievements row merged with static metadata. The metadata keys
 * (name/description/icon/rarity) are structurally optional in the merge but
 * always present for the fixed 5-type set. nft_* columns are legacy:
 * nft_minted is the SQLite integer 0/1, never a JSON boolean.
 */
export interface Achievement {
  id: number;
  achievement_type: AchievementType;
  /** SqliteTimestamp; normalized to "<TS>" in fixtures. */
  earned_at: SqliteTimestamp;
  nft_minted: number;
  nft_token_id: number | null;
  nft_tx_hash: string | null;
  name: string;
  description: string;
  /**
   * Lucide icon name — backend truth (consistency_king="Target",
   * mood_master="Crown"; the frontend's own hardcoded map disagrees).
   */
  icon: string;
  rarity: AchievementRarity;
}

/**
 * Element of new_achievements in both POST /api/achievements/check and
 * POST /api/mood (shapes unified in the rewrite): metadata only, WITHOUT
 * id/earned_at — re-fetch GET /api/achievements for the full rows.
 */
export interface NewAchievement {
  achievement_type: AchievementType;
  name: string;
  description: string;
  icon: string;
  rarity: AchievementRarity;
}

/** POST /api/achievements/check 200 body (idempotent; [] on re-check). */
export interface CheckAchievementsResponse {
  new_achievements: NewAchievement[];
  /** Always new_achievements.length. */
  count: number;
}

export interface AchievementProgressEntry {
  /** Clamped to [0, max]. */
  current: number;
  max: number;
}

/**
 * GET /api/achievements/progress 200 body: exactly these five keys, always
 * all present. Fixed maxima: first_entry 1, week_warrior 7,
 * consistency_king 30, data_lover 10, mood_master 100.
 */
export interface AchievementProgress {
  first_entry: AchievementProgressEntry;
  week_warrior: AchievementProgressEntry;
  consistency_king: AchievementProgressEntry;
  data_lover: AchievementProgressEntry;
  mood_master: AchievementProgressEntry;
}

// ---------------------------------------------------------------------------
// Misc (health, time, music, export)
// ---------------------------------------------------------------------------

/** GET /api/ 200 body (note the trailing slash — bare /api 308-redirects). */
export interface Health {
  status: 'healthy';
  message: string;
  /** Float epoch seconds. */
  timestamp: number;
}

/** GET /api/time 200 body. */
export interface ServerTime {
  /** Float epoch seconds. */
  time: number;
}

/**
 * GET /api/music/vibe 200 body — only when ENABLE_MOOD_MUSIC is on; with the
 * flag off the path 404s like any unknown route.
 */
export interface MusicTrack {
  audio_url: string;
  track_name: string;
  artist: string;
}

/** POST /api/export/pdf request body (response is a binary PDF, not JSON). */
export interface ExportPdfRequest {
  /** Markdown source; at most 1 MiB of UTF-8 bytes (413 above). */
  content: string;
}
