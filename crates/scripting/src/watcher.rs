use std::sync::Arc;
use std::time::Duration;

use notify::event::EventKind;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{info, warn};

use tomo_core::error::{Error, Result};

use crate::manager::ScriptManager;

/// Spawn a notify-based watcher that triggers `manager.reload()` whenever any
/// `.rhai` file under the script directory is created, modified or removed.
/// The returned watcher must be kept alive for the lifetime of the bot —
/// dropping it stops the hot-reload.
pub fn spawn(manager: &Arc<ScriptManager>) -> Result<RecommendedWatcher> {
    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();

    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| Error::config(format!("notify watcher: {e}")))?;

    watcher
        .watch(manager.root(), RecursiveMode::Recursive)
        .map_err(|e| Error::config(format!("watch {}: {e}", manager.root().display())))?;

    info!(path = ?manager.root(), "script hot-reload enabled");

    let mgr = Arc::clone(manager);
    tokio::spawn(async move {
        const DEBOUNCE: Duration = Duration::from_millis(200);
        loop {
            let Some(first) = rx.recv().await else { return };
            if !is_relevant(&first) {
                continue;
            }
            // Collapse the burst of events editors fire on save.
            tokio::time::sleep(DEBOUNCE).await;
            while let Ok(_extra) = rx.try_recv() {}

            if let Err(e) = mgr.reload().await {
                warn!(error = %e, "script reload failed");
            }
        }
    });

    Ok(watcher)
}

fn is_relevant(res: &notify::Result<notify::Event>) -> bool {
    let Ok(ev) = res else { return false };
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && ev
        .paths
        .iter()
        .any(|p| p.extension().map(|e| e == "rhai").unwrap_or(false))
}
