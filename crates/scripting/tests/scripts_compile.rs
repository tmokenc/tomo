//! Smoke-tests for every shipped script under `scripts/{commands,triggers}`:
//!
//! 1. `every_command_script_compiles_and_meta_is_a_map` — the script
//!    parses, defines `meta()`, and `meta()` returns a map. Catches typos
//!    and missing function definitions.
//! 2. `every_command_script_executes_without_error` — `execute(ctx)` runs
//!    to completion with a synthetic context. This is what pinned the
//!    `parse_int` regression: a script that compiled fine threw at runtime
//!    on empty/non-numeric input, and the only way to learn that was
//!    invoking it in Discord and watching the bot warn-log.

use std::fs;
use std::path::{Path, PathBuf};

use rhai::{Dynamic, Map, Scope};

use tomo_scripting::{ScriptCtx, ScriptInit};

/// `<workspace>/scripts` — discovered relative to this crate's manifest dir.
fn scripts_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("scripts")
}

fn collect(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("rhai") {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn every_command_script_compiles_and_meta_is_a_map() {
    let engine = tomo_scripting::engine::make_engine();
    let root = scripts_root();
    let cmd_dir = root.join("commands");
    let trig_dir = root.join("triggers");

    let mut failures = Vec::new();

    for path in collect(&cmd_dir).into_iter().chain(collect(&trig_dir)) {
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("read {}: {e}", path.display()));
                continue;
            }
        };
        let ast = match engine.compile(&source) {
            Ok(ast) => ast,
            Err(e) => {
                failures.push(format!("compile {}: {e}", path.display()));
                continue;
            }
        };

        let mut scope = Scope::new();
        let meta: Result<Dynamic, _> = engine.call_fn(&mut scope, &ast, "meta", ());
        match meta {
            Ok(d) => {
                if d.try_cast::<Map>().is_none() {
                    failures.push(format!(
                        "{}: meta() did not return a map",
                        path.display()
                    ));
                }
            }
            Err(e) => failures.push(format!("meta() {}: {e}", path.display())),
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} script(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

/// Each command script's `execute(ctx)` runs to completion with a few
/// representative argument values. Triggers are not exercised here — they
/// don't define an entry point of their own and run through the same
/// machinery once a real message matches the matcher.
#[test]
fn every_command_script_executes_without_error() {
    let engine = tomo_scripting::engine::make_engine();
    let root = scripts_root();
    let cmd_dir = root.join("commands");

    // Inputs picked to exercise the parse paths each script branches on.
    const INPUTS: &[&str] = &[
        "",            // no args (random→float, roll→default, time→now, etc.)
        "5",           // integer
        "1 10",        // two-arg numeric range (random)
        "foo|bar|baz", // pipe list (choose)
        "hi there",    // freeform text (echo, etc.)
    ];

    let mut failures = Vec::new();

    for path in collect(&cmd_dir) {
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("read {}: {e}", path.display()));
                continue;
            }
        };
        let ast = match engine.compile(&source) {
            Ok(ast) => ast,
            Err(e) => {
                failures.push(format!("compile {}: {e}", path.display()));
                continue;
            }
        };

        for &input in INPUTS {
            let (ctx, _rx) = ScriptCtx::new(ScriptInit {
                channel_id: 1,
                guild_id: Some(1),
                user_id: 1,
                message_id: 1,
                args: input.into(),
                author_name: "tester".into(),
                author_avatar_url: None,
                bot_name: "Tomo".into(),
                bot_avatar_url: None,
                bot_started_at_unix: 0,
                now_unix: 1_700_000_000,
            });

            let mut scope = Scope::new();
            if let Err(e) = engine.call_fn::<Dynamic>(&mut scope, &ast, "execute", (ctx,)) {
                failures.push(format!(
                    "execute() in {} with args={input:?}: {e}",
                    path.display()
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} script execution(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
