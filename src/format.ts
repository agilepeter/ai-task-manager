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

/// Whole days between an earlier instant and now, floor-divided so any
/// time-of-day within the same calendar span still lands in bucket 0
/// ("today"). The one place that arithmetic lives, so relativeActivity()'s
/// full sentence below and relativeDay()'s bare label can never disagree
/// about where "today" ends, even though they read from two different
/// families of locale keys. `nowMs` is always a parameter, never
/// `Date.now()` read in here, so a test can pick any "now" and get a
/// stable, repeatable answer.
function dayBucket(nowMs: number, thenMs: number): number {
  return Math.floor((nowMs - thenMs) / 86_400_000);
}

/// "active today" / "active yesterday" / "last active {n} days ago": the
/// sessions list's own full sentence (src/detail.ts's lastActive()).
export function relativeActivity(ms: number, nowMs: number): string {
  const days = dayBucket(nowMs, ms);
  if (days <= 0) return t("detail.session.activeToday");
  if (days === 1) return t("detail.session.activeYesterday");
  return plural("detail.session.lastActive", days);
}

/// "today" / "yesterday" / "{count} days ago": a bare label for a caller
/// that wraps it in its own sentence, e.g. describeAgentSpend()'s
/// (src/inventory.ts) "last used {when}" -- splicing relativeActivity()'s
/// full "active today" into that slot is how the sentence used to read
/// "last used active today" in the first place. Not the same keys as the
/// `time.today` / `time.tomorrow` pair above either: those always pair a
/// day with a clock time ("today at {time}") for a different feature
/// (main.ts/detail.ts's exact-timestamp formatting), and reusing them bare
/// here would silently drop that time at their other call sites.
export function relativeDay(nowMs: number, thenMs: number): string {
  const days = dayBucket(nowMs, thenMs);
  if (days <= 0) return t("time.relativeToday");
  if (days === 1) return t("time.relativeYesterday");
  return plural("time.relativeDaysAgo", days);
}
