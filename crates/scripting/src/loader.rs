use std::path::{Path, PathBuf};
use std::sync::Arc;

use rhai::{Dynamic, Engine, Map, Scope, AST};
use tracing::{error, info, warn};

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
        let scripts = walk_rhai(&cmd_dir)?;
        info!(count = scripts.len(), dir = %cmd_dir.display(), "scanning command scripts");
        for entry in scripts {
            match load_command(engine, &entry) {
                Ok(cmd) => {
                    info!(
                        name = %cmd.name,
                        aliases = ?cmd.aliases,
                        path = %cmd.source_path.display(),
                        "loaded command script"
                    );
                    for alias in &cmd.aliases {
                        registry.commands.insert(alias.to_lowercase(), cmd.clone());
                    }
                    registry.commands.insert(cmd.name.to_lowercase(), cmd);
                }
                Err(e) => warn!(path = %entry.display(), error = %e, "skipping command script"),
            }
        }
    } else {
        warn!(dir = %cmd_dir.display(), "scripts/commands directory not found");
    }

    let trigger_dir = root.join("triggers");
    if trigger_dir.is_dir() {
        let scripts = walk_rhai(&trigger_dir)?;
        info!(count = scripts.len(), dir = %trigger_dir.display(), "scanning trigger scripts");
        for entry in scripts {
            match load_trigger(engine, &entry) {
                Ok(t) => {
                    info!(name = %t.name, path = %t.source_path.display(), "loaded trigger script");
                    registry.triggers.push(t);
                }
                Err(e) => warn!(path = %entry.display(), error = %e, "skipping trigger script"),
            }
        }
    } else {
        warn!(dir = %trigger_dir.display(), "scripts/triggers directory not found");
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
    let category = meta_string(&meta, "category");

    Ok(ScriptCommand {
        name,
        description,
        aliases,
        category,
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
        .and_then(|v| v.clone().try_cast::<Map>())
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
    // Rhai stores strings as `ImmutableString` internally, so
    // `read_lock::<String>()` returns `None` even for string-valued keys.
    // `try_cast::<String>()` has a special-case for `Union::Str` that does
    // the conversion. We clone the borrow because `try_cast` consumes its
    // receiver.
    meta.get(key).and_then(|v| v.clone().try_cast::<String>())
}

fn meta_bool(meta: &Map, key: &str) -> Option<bool> {
    meta.get(key).and_then(|v| v.as_bool().ok())
}

fn meta_string_array(meta: &Map, key: &str) -> Vec<String> {
    meta.get(key)
        .and_then(|v| v.clone().try_cast::<rhai::Array>())
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| v.try_cast::<String>())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{meta_bool, meta_string, meta_string_array};
    use rhai::{Engine, Map, Scope};

    /// Round-trip: parse a script that has the same `meta()` shape used by
    /// real command files, then read it back through `meta_string` /
    /// `meta_string_array` / `meta_bool`. This is the exact path that broke
    /// silently before — `meta_string` was returning `None` for string keys
    /// because Rhai stores strings as `ImmutableString` and `read_lock::<String>`
    /// can't unwrap them.
    #[test]
    fn meta_helpers_round_trip_strings_and_arrays() {
        let engine = Engine::new();
        let source = r#"
            fn meta() {
                #{
                    name: "coin",
                    description: "Flip a coin.",
                    aliases: ["flip"],
                    slash: false,
                }
            }
        "#;
        let ast = engine.compile(source).expect("compile");
        let mut scope = Scope::new();
        let meta: Map = engine
            .call_fn(&mut scope, &ast, "meta", ())
            .expect("meta()");
        assert_eq!(meta_string(&meta, "name").as_deref(), Some("coin"));
        assert_eq!(meta_string(&meta, "description").as_deref(), Some("Flip a coin."));
        assert_eq!(meta_string_array(&meta, "aliases"), vec!["flip".to_string()]);
        assert_eq!(meta_bool(&meta, "slash"), Some(false));
        // Missing keys remain None.
        assert_eq!(meta_string(&meta, "absent"), None);
        assert_eq!(meta_bool(&meta, "absent"), None);
        assert!(meta_string_array(&meta, "absent").is_empty());
    }
}
