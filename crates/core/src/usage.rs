//! A plain one-shot fetch of live limits, for callers without the tray app's
//! refresh machinery (the headless agent). Only tools that sign in through a
//! credential already on this machine: nothing here needs a key pasted into
//! the app. The tray app keeps its own richer path (caching, back-off,
//! per-account cards); this is deliberately the simple one.

use crate::providers::{self, Snapshot};

pub async fn local_credential_snapshots() -> Vec<Snapshot> {
    let (claude, codex, copilot, cursor) = tokio::join!(
        providers::claude::snapshot(),
        providers::codex::snapshot(),
        providers::copilot::snapshot(),
        providers::cursor::snapshot(),
    );
    vec![claude, codex, copilot, cursor]
}
