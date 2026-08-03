// The Rust surface, typed once.
//
// These types mirror the `#[derive(Serialize)]` structs in src/app/commands.rs. There is
// no generator: three structs and a hand-written mirror is cheaper to read than a build
// step, and a drift shows up immediately as a compile error in the view that uses it.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface RateWindow {
  label: string;
  used_percent: number;
  resets_at?: string;
  window_minutes?: number;
}

export interface Credits {
  balance?: number;
  unlimited: boolean;
  has_credits: boolean;
}

export interface UsageSnapshot {
  provider: string;
  plan?: string;
  account_label?: string;
  windows: RateWindow[];
  credits?: Credits;
  fetched_at: string;
}

export interface ProviderView {
  id: string;
  name: string;
  snapshot: UsageSnapshot | null;
  error: string | null;
  needsUserAction: boolean;
}

export interface Overview {
  providers: ProviderView[];
  refreshing: boolean;
}

export interface Reading {
  at: string;
  provider: string;
  label: string;
  used_percent: number;
}

export interface ProviderSetting {
  id: string;
  name: string;
  enabled: boolean;
  apiKey: string | null;
  takesApiKey: boolean;
  /// A second field, for a provider that scopes its key to a team in the URL.
  workspaceId: string | null;
  takesWorkspaceId: boolean;
  workspaceHint: string;
  /// A pasted Cookie header, for providers whose usage is only on the dashboard.
  cookieHeader: string | null;
  takesCookie: boolean;
  hint: string;
}

export type CostGroup =
  | "model"
  | "provider"
  | "harness"
  | "session"
  | "hour"
  | "day"
  | "week"
  | "month"
  | "workspace";

export interface CostRow {
  key: string;
  messages: number;
  tokens: number;
  /** null when too little of the row could be priced for a number to mean anything. */
  costUsd: number | null;
  /** Share of this row's tokens carrying a price, 0..1. */
  coverage: number;
  /// Cache reads as a share of this row's tokens.
  cacheRatio: number | null;
  /// Blended dollars per million priced tokens.
  perMillion: number | null;
  /// This row's share of the report total.
  share: number;
}

export interface AgentRow {
  name: string;
  present: boolean;
  files: number;
  messages: number;
}

export interface DayPoint {
  date: string;
  messages: number;
  tokens: number;
  costUsd: number | null;
}

/// The four kinds of token, billed at four different rates.
export interface TokenMix {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  reasoning: number;
}

/// Where axio keeps its files.
export interface Storage {
  configPath: string;
  historyPath: string;
  scan: {
    path: string;
    bytes: number;
    scannedAt: string | null;
    scanning: boolean;
  };
}

export interface Stats {
  days: DayPoint[];
  busiestDayTokens: number;
  /// The three cuts that split a day into four heat levels, from the Rust side so the
  /// window and the CLI shade a day the same way.
  thresholds: [number, number, number];
  byHour: number[];
  byWeekday: number[];
  byProvider: CostRow[];
  byHarness: CostRow[];
  byWorkspace: CostRow[];
  mix: TokenMix;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
  sessions: number;
  topModel: string | null;
  totalTokens: number;
  totalCostUsd: number | null;
}

export interface CostReport {
  rows: CostRow[];
  total: CostRow;
  unpricedModels: string[];
  agents: AgentRow[];
  loading: boolean;
  /// A scan is running behind these figures. They are real, just about to be replaced.
  scanning: boolean;
}

export const api = {
  overview: () => invoke<Overview>("overview"),
  refresh: () => invoke<void>("refresh_now"),
  history: () => invoke<Reading[]>("history"),
  settings: () => invoke<ProviderSetting[]>("settings"),
  saveSettings: (settings: ProviderSetting[]) =>
    invoke<string>("save_settings", { settings }),
  costReport: (group: CostGroup) => invoke<CostReport>("cost_report", { group }),
  refreshCost: () => invoke<void>("refresh_cost"),
  costStats: () => invoke<Stats>("cost_stats"),
  storage: () => invoke<Storage>("storage"),
  refreshCadence: () => invoke<string>("refresh_cadence"),
  setRefreshCadence: (cadence: string) =>
    invoke<string>("set_refresh_cadence", { cadence }),
  openMainWindow: () => invoke<void>("open_main_window"),
  minimizeWindow: () => invoke<void>("minimize_window"),
  closeWindow: () => invoke<void>("close_window"),
  hideFlyout: () => invoke<void>("hide_flyout"),
  quit: () => invoke<void>("quit"),
};

/// Fires whenever a probe finishes, so neither surface has to poll.
export const onUpdated = (handler: () => void) =>
  listen("quota://updated", handler);

/// Fires when the cost scan publishes: once from the saved scan, once from the live one.
///
/// Listening rather than polling, because those two arrive seconds apart at launch and an
/// interval short enough to catch the second would then run for the rest of the session.
export const onCostUpdated = (handler: () => void) =>
  listen("cost://updated", handler);

/// Severity bands, kept identical to `Severity::from_used_percent` in Rust. Duplicated
/// deliberately: the alternative is a round trip to compute a CSS class.
export function severity(usedPercent: number): "" | "warn" | "crit" {
  if (usedPercent >= 90) return "crit";
  if (usedPercent >= 75) return "warn";
  return "";
}

/// Time remaining, not a timestamp — "resets in 5d" is the question people actually ask.
export function resetText(resetsAt?: string): string {
  if (!resetsAt) return "";
  const remainingMs = new Date(resetsAt).getTime() - Date.now();
  if (remainingMs <= 0) return "resetting";
  const minutes = Math.floor(remainingMs / 60000);
  if (minutes < 60) return `resets in ${Math.max(1, minutes)}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `resets in ${hours}h`;
  return `resets in ${Math.floor(hours / 24)}d`;
}

/// How far through a quota window we are, 0..1, or null when the log does not say.
///
/// Needs both the reset time and the nominal window length: the first gives the end, the
/// second gives the span, and the elapsed fraction is what is left over. A window missing
/// either is drawn without a tick rather than with a guessed one.
export function elapsedFraction(window: RateWindow, now = Date.now()): number | null {
  if (!window.resets_at || !window.window_minutes) return null;
  const resetsAt = Date.parse(window.resets_at);
  if (Number.isNaN(resetsAt)) return null;

  const spanMs = window.window_minutes * 60_000;
  if (spanMs <= 0) return null;

  const remaining = (resetsAt - now) / spanMs;
  return Math.min(1, Math.max(0, 1 - remaining));
}

/// Whether consumption is running ahead of the clock.
///
/// The reading the rail exists for: quota refills on a timer, so spending it faster than
/// the timer runs means hitting the limit before the window turns over. A five point
/// tolerance keeps the label from flickering between states on every refresh.
export function pace(
  window: RateWindow,
): { over: boolean; label: string } | null {
  const elapsed = elapsedFraction(window);
  if (elapsed === null) return null;

  const gap = window.used_percent - elapsed * 100;
  if (Math.abs(gap) < 5) return { over: false, label: "on pace" };
  return gap > 0
    ? { over: true, label: `${Math.round(gap)}pt ahead of clock` }
    : { over: false, label: `${Math.round(-gap)}pt spare` };
}
