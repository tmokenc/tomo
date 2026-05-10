use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rhai::{Engine, Scope};
use tokio::task;
use tracing::{debug, error, info};

use tomo_core::error::{Error, Result};

use crate::ctx::{ScriptAction, ScriptCtx};
use crate::engine::make_engine;
use crate::loader::load_all;
use crate::registry::{ScriptCommand, ScriptRegistry, ScriptTrigger};

/// Owns the Rhai engine and the current registry of compiled scripts. Reload
/// is atomic: callers always observe a single coherent registry snapshot.
pub struct ScriptManager {
    engine: Arc<Engine>,
    root: PathBuf,
    registry: ArcSwap<ScriptRegistry>,
}

impl ScriptManager {
    pub fn new(script_dir: impl AsRef<Path>) -> Self {
        Self {
            engine: Arc::new(make_engine()),
            root: script_dir.as_ref().to_path_buf(),
            registry: ArcSwap::from_pointee(ScriptRegistry::default()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn registry(&self) -> Arc<ScriptRegistry> {
        self.registry.load_full()
    }

    /// Recompile every script under `root`. Errors on individual files are
    /// logged but do not abort the reload.
    pub async fn reload(&self) -> Result<()> {
        info!(path = ?self.root, "reloading scripts");
        let engine = Arc::clone(&self.engine);
        let root = self.root.clone();
        let registry = task::spawn_blocking(move || load_all(&engine, &root))
            .await
            .map_err(|e| Error::script(format!("reload join: {e}")))??;
        debug!(
            commands = registry.commands.len(),
            triggers = registry.triggers.len(),
            "scripts reloaded"
        );
        self.registry.store(Arc::new(registry));
        Ok(())
    }

    pub async fn run_command(&self, cmd: &ScriptCommand, ctx: ScriptCtx, rx: flume::Receiver<ScriptAction>) -> Result<Vec<ScriptAction>> {
        self.run(Arc::clone(&cmd.ast), ctx, rx).await
    }

    pub async fn run_trigger(&self, trig: &ScriptTrigger, ctx: ScriptCtx, rx: flume::Receiver<ScriptAction>) -> Result<Vec<ScriptAction>> {
        self.run(Arc::clone(&trig.ast), ctx, rx).await
    }

    async fn run(
        &self,
        ast: Arc<rhai::AST>,
        ctx: ScriptCtx,
        rx: flume::Receiver<ScriptAction>,
    ) -> Result<Vec<ScriptAction>> {
        let engine = Arc::clone(&self.engine);
        let join = task::spawn_blocking(move || {
            let mut scope = Scope::new();
            let _ignored: rhai::Dynamic = engine
                .call_fn(&mut scope, &ast, "execute", (ctx,))
                .map_err(|e| Error::script(format!("execute(): {e}")))?;
            Ok::<(), Error>(())
        });

        match join.await {
            Ok(Ok(())) => Ok(rx.drain().collect()),
            Ok(Err(e)) => {
                error!(error = %e, "script execute() failed");
                Err(e)
            }
            Err(join_err) => Err(Error::script(format!("script panic: {join_err}"))),
        }
    }
}
