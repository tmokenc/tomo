use std::path::{Path, PathBuf};
use std::sync::Arc;

use rhai::{Dynamic, Engine, Map, Scope, AST};
use tracing::{debug, error, warn};

use tomo_core::error::{Error, Result};

use crate::registry::{ScriptCommand, ScriptRegistry, ScriptTrigger};
use crate::trigger::TriggerMatcher;

/// Compile every `.rhai` file under `<root>/commands` and `<root>/triggers`
/// into a fresh [`ScriptRegistry`]. Failures on individual files are logged
/// and skipped so a typo in one script does not take the whole bot down.
pub fn load_all(engine: &Engine, root: &Path) -> Result<ScriptRegistry> {
    let mut registry = ScriptRegistry::default();

    let cmd_dir = root.join("commands");
    if cmd_dir.is_dir() {
        for entry in walk_rhai(&cmd_dir)? {
            match load_command(engine, &entry) {
                Ok(cmd) => {
                    debug!(name = %cmd.name, path = ?cmd.source_path, "loaded command script");
                    for alias in &cmd.aliases {
                        registry.commands.insert(alias.to_lowercase(), cmd.clone());
                    }
                    registry.commands.insert(cmd.name.to_lowercase(), cmd);
                }
                Err(e) => warn!(path = ?entry, error = %e, "skipping command script"),
            }
        }
    }

    let trigger_dir = root.join("triggers");
    if trigger_dir.is_dir() {
        for entry in walk_rhai(&trigger_dir)? {
            match load_trigger(engine, &entry) {
                Ok(t) => {
                    debug!(name = %t.name, path = ?t.source_path, "loaded trigger script");
                    registry.triggers.push(t);
                }
                Err(e) => warn!(path = ?entry, error = %e, "skipping trigger script"),
            }
        }
    }

    Ok(registry)
}

fn walk_rhai(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let read = std::fs::read_dir(dir).map_err(Error::from)?;
    for entry in read {
        let entry = entry.map_err(Error::from)?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "rhai").unwrap_or(false) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn load_command(engine: &Engine, path: &Path) -> Result<ScriptCommand> {
    let (ast, meta) = compile_and_meta(engine, path)?;
    let name = meta_string(&meta, "name").ok_or_else(|| {
        Error::script(format!("{}: meta() is missing `name`", path.display()))
    })?;
    let description = meta_string(&meta, "description").unwrap_or_default();
    let slash = meta_bool(&meta, "slash").unwrap_or(true);
    let prefix = meta_bool(&meta, "prefix").unwrap_or(true);
    let guild_only = meta_bool(&meta, "guild_only").unwrap_or(false);
    let aliases = meta_string_array(&meta, "aliases");

    Ok(ScriptCommand {
        name,
        description,
        aliases,
        slash,
        prefix,
        guild_only,
        source_path: path.to_path_buf(),
        ast: Arc::new(ast),
    })
}

fn load_trigger(engine: &Engine, path: &Path) -> Result<ScriptTrigger> {
    let (ast, meta) = compile_and_meta(engine, path)?;
    let name = meta_string(&meta, "name").ok_or_else(|| {
        Error::script(format!("{}: meta() is missing `name`", path.display()))
    })?;
    let match_meta = meta
        .get("match")
        .and_then(|v| v.read_lock::<Map>().map(|m| m.clone()))
        .ok_or_else(|| Error::script(format!("{}: meta() is missing `match`", path.display())))?;
    let matcher = TriggerMatcher::from_meta(&match_meta)?;

    Ok(ScriptTrigger {
        name,
        matcher,
        source_path: path.to_path_buf(),
        ast: Arc::new(ast),
    })
}

fn compile_and_meta(engine: &Engine, path: &Path) -> Result<(AST, Map)> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        Error::script(format!("read {}: {e}", path.display()))
    })?;
    let ast = engine.compile(&source).map_err(|e| {
        error!(path = ?path, error = %e, "rhai compile error");
        Error::script(format!("compile {}: {e}", path.display()))
    })?;

    let mut scope = Scope::new();
    let meta_dyn: Dynamic = engine
        .call_fn(&mut scope, &ast, "meta", ())
        .map_err(|e| Error::script(format!("meta() in {}: {e}", path.display())))?;

    let meta = meta_dyn
        .try_cast::<Map>()
        .ok_or_else(|| Error::script(format!("{}: meta() must return a map", path.display())))?;

    Ok((ast, meta))
}

fn meta_string(meta: &Map, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(|v| v.read_lock::<String>().map(|s| s.clone()))
}

fn meta_bool(meta: &Map, key: &str) -> Option<bool> {
    meta.get(key).and_then(|v| v.as_bool().ok())
}

fn meta_string_array(meta: &Map, key: &str) -> Vec<String> {
    meta.get(key)
        .and_then(|v| v.read_lock::<rhai::Array>().map(|a| a.clone()))
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| v.try_cast::<String>())
                .collect()
        })
        .unwrap_or_default()
}
