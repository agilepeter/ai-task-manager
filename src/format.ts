// Number and time formatting shared by every view that prints a dollar
// amount, a token count or "how long since" -- Usage (src/detail.ts),
// Subscriptions (src/ledger.ts) and Inventory (src/inventory.ts). One copy
// so those views can never drift onto different rounding, grouping or
// wording rules for the same numbers.

import { localeTag, plural, t } from "./i18n";

/// Whole dollars from $10 up, cents below: a column of values then reads at
/// one precision per magnitude instead of "$929" beside "$11.0".
/// $ stays a symbol, but the digit grouping follows the app's language, not the OS's.
export function money(n: number): string {
  return n >= 10 ? `$${Math.round(n).toLocaleString(localeTag())}` : `$${n.toFixed(2)}`;
}

/// B/M/K stay English: format tokens, not prose, like money()'s $ and fileSize()'s
/// MB/KB — "tokens" itself is this codebase's house loanword in every non-English
/// locale's strings.
export function tokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)}K`;
  return String(Math.round(n));
}

/// "active today" / "active yesterday" / "last active {n} days ago": one
/// day-bucketing rule for "how long since X happened", shared by the
/// sessions list (src/detail.ts's lastActive()) and the per-agent spend
/// line (src/inventory.ts's describeAgentSpend()) so the two can never
/// disagree about where "today" ends. `nowMs` is a parameter, never
/// `Date.now()` read in here, so a test can pick any "now" and get a
/// stable, repeatable answer.
export function relativeActivity(ms: number, nowMs: number): string {
  const days = Math.floor((nowMs - ms) / 86_400_000);
  if (days <= 0) return t("detail.session.activeToday");
  if (days === 1) return t("detail.session.activeYesterday");
  return plural("detail.session.lastActive", days);
}
