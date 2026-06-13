//! Wire types — match the shapes produced by `tomo-admin/src/api.rs`.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Me {
    pub user_id: u64,
    pub username: String,
    pub expires_at_unix: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Status {
    pub name: String,
    pub version: String,
    pub started_at_unix: i64,
    pub uptime_seconds: i64,
    pub owners: Vec<u64>,
    pub bot_user_id: u64,
    pub llm_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct GlobalStats {
    pub messages: u64,
    pub commands: u64,
    pub llm_calls: u64,
    pub llm_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TopCommands {
    pub commands: Vec<TopCommandEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TopCommandEntry {
    pub command: String,
    pub count: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub slash: bool,
    pub prefix: bool,
    pub guild_only: bool,
    pub owner_only: bool,
    pub is_script: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TriggerInfo {
    pub name: String,
    pub matcher_kind: String,
    pub matcher_arg: String,
    pub is_script: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ReloadResult {
    pub success: bool,
    pub message: String,
    pub command_count: u32,
    pub trigger_count: u32,
}
