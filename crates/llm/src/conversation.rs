use std::collections::VecDeque;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// Per-channel ring buffer of recent turns. Older turns are dropped silently
/// once the buffer fills.
pub struct ConversationStore {
    capacity: usize,
    inner: DashMap<Id<ChannelMarker>, VecDeque<Turn>>,
}

impl ConversationStore {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, inner: DashMap::new() }
    }

    pub fn push(&self, channel: Id<ChannelMarker>, turn: Turn) {
        let mut entry = self.inner.entry(channel).or_default();
        if entry.len() >= self.capacity {
            entry.pop_front();
        }
        entry.push_back(turn);
    }

    pub fn snapshot(&self, channel: Id<ChannelMarker>) -> Vec<Turn> {
        self.inner
            .get(&channel)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self, channel: Id<ChannelMarker>) {
        self.inner.remove(&channel);
    }
}
