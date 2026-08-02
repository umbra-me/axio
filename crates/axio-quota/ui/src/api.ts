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
  hint: string;
}

export interface CostSource {
  name: string;
  path: string;
  exists: boolean;
}

export const api = {
  overview: () => invoke<Overview>("overview"),
  refresh: () => invoke<void>("refresh_now"),
  history: () => invoke<Reading[]>("history"),
  settings: () => invoke<ProviderSetting[]>("settings"),
  saveSettings: (settings: ProviderSetting[]) =>
    invoke<string>("save_settings", { settings }),
  costSources: () => invoke<CostSource[]>("cost_sources"),
  openMainWindow: () => invoke<void>("open_main_window"),
  hideFlyout: () => invoke<void>("hide_flyout"),
  quit: () => invoke<void>("quit"),
};

/// Fires whenever a probe finishes, so neither surface has to poll.
export const onUpdated = (handler: () => void) =>
  listen("quota://updated", handler);

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
