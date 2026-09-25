// Number formatting shared by every view that prints a dollar amount or a
// token count -- Usage (src/detail.ts), Subscriptions (src/ledger.ts) and
// Inventory (src/inventory.ts). One copy so the three views can never drift
// onto different rounding, grouping or magnitude rules for the same numbers.

import { localeTag } from "./i18n";

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
