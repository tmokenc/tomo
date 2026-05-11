//! Permission calculation built on top of the cache.
//!
//! Mirror of `twilight_cache_inmemory::permission`. Gated by the
//! `permission-calculator` feature so the (optional) `twilight-util`
//! dependency only gets pulled in when callers actually want it.
//!
//! # Required Configuration
//!
//! Calculating permissions reads channels, members, and roles from the cache.
//! Make sure those resource types are enabled:
//!
//! ```
//! use tomo_cache::ResourceType;
//!
//! let _ = ResourceType::CHANNEL | ResourceType::MEMBER | ResourceType::ROLE;
//! ```
//!
//! # Disabled Member Communication
//!
//! If a member's `communication_disabled_until` is in the future, computed
//! permissions are intersected with [`MEMBER_COMMUNICATION_DISABLED_ALLOWLIST`]
//! unless the calculator was configured otherwise via
//! [`PersistentCachePermissions::check_member_communication_disabled`].
//! Administrators are never disabled.

use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::time::{Duration, SystemTime};

use twilight_model::channel::permission_overwrite::PermissionOverwrite;
use twilight_model::channel::{Channel, ChannelType};
use twilight_model::guild::{Member, Permissions};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker};
use twilight_util::permission_calculator::PermissionCalculator;

use crate::PersistentCache;

/// Permissions kept when a member's communication has been disabled.
pub const MEMBER_COMMUNICATION_DISABLED_ALLOWLIST: Permissions = Permissions::from_bits_truncate(
    Permissions::READ_MESSAGE_HISTORY.bits() | Permissions::VIEW_CHANNEL.bits(),
);

// -------------------- error types --------------------

#[derive(Debug)]
pub struct ChannelError {
    kind: ChannelErrorType,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl ChannelError {
    pub const fn kind(&self) -> &ChannelErrorType {
        &self.kind
    }

    pub fn into_source(self) -> Option<Box<dyn Error + Send + Sync>> {
        self.source
    }

    pub fn into_parts(self) -> (ChannelErrorType, Option<Box<dyn Error + Send + Sync>>) {
        (self.kind, self.source)
    }

    fn from_member_roles(e: MemberRolesErrorType) -> Self {
        let MemberRolesErrorType::RoleMissing { role_id } = e;
        Self { kind: ChannelErrorType::RoleUnavailable { role_id }, source: None }
    }
}

impl Display for ChannelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self.kind {
            ChannelErrorType::ChannelNotInGuild { channel_id } => {
                write!(f, "channel {channel_id} is not in a guild")
            }
            ChannelErrorType::ChannelUnavailable { channel_id } => {
                write!(f, "channel {channel_id} is either not cached or not a guild channel")
            }
            ChannelErrorType::MemberUnavailable { guild_id, user_id } => {
                write!(f, "member (guild: {guild_id}; user: {user_id}) is not cached")
            }
            ChannelErrorType::ParentChannelNotPresent { thread_id } => {
                write!(f, "thread {thread_id} has no parent")
            }
            ChannelErrorType::RoleUnavailable { role_id } => {
                write!(f, "member has role {role_id} but it is not cached")
            }
        }
    }
}

impl Error for ChannelError {}

#[derive(Debug)]
#[non_exhaustive]
pub enum ChannelErrorType {
    ChannelNotInGuild { channel_id: Id<ChannelMarker> },
    ChannelUnavailable { channel_id: Id<ChannelMarker> },
    MemberUnavailable { guild_id: Id<GuildMarker>, user_id: Id<UserMarker> },
    ParentChannelNotPresent { thread_id: Id<ChannelMarker> },
    RoleUnavailable { role_id: Id<RoleMarker> },
}

#[derive(Debug)]
pub struct RootError {
    kind: RootErrorType,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl RootError {
    pub const fn kind(&self) -> &RootErrorType {
        &self.kind
    }

    pub fn into_source(self) -> Option<Box<dyn Error + Send + Sync>> {
        self.source
    }

    pub fn into_parts(self) -> (RootErrorType, Option<Box<dyn Error + Send + Sync>>) {
        (self.kind, self.source)
    }

    fn from_member_roles(e: MemberRolesErrorType) -> Self {
        let MemberRolesErrorType::RoleMissing { role_id } = e;
        Self { kind: RootErrorType::RoleUnavailable { role_id }, source: None }
    }
}

impl Display for RootError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self.kind {
            RootErrorType::MemberUnavailable { guild_id, user_id } => {
                write!(f, "member (guild: {guild_id}; user: {user_id}) is not cached")
            }
            RootErrorType::RoleUnavailable { role_id } => {
                write!(f, "member has role {role_id} but it is not cached")
            }
        }
    }
}

impl Error for RootError {}

#[derive(Debug)]
#[non_exhaustive]
pub enum RootErrorType {
    MemberUnavailable { guild_id: Id<GuildMarker>, user_id: Id<UserMarker> },
    RoleUnavailable { role_id: Id<RoleMarker> },
}

enum MemberRolesErrorType {
    RoleMissing { role_id: Id<RoleMarker> },
}

/// Member's roles' permissions plus the guild's `@everyone` role.
struct MemberRoles {
    assigned: Vec<(Id<RoleMarker>, Permissions)>,
    everyone: Permissions,
}

// -------------------- the calculator --------------------

/// Compute a member's permissions using cached channels, members, and roles.
/// Mirror of `InMemoryCachePermissions`; reachable via
/// [`PersistentCache::permissions`].
#[derive(Clone, Debug)]
#[must_use = "has no effect if unused"]
pub struct PersistentCachePermissions<'a> {
    cache: &'a PersistentCache,
    check_member_communication_disabled: bool,
}

impl<'a> PersistentCachePermissions<'a> {
    pub(crate) const fn new(cache: &'a PersistentCache) -> Self {
        Self { cache, check_member_communication_disabled: true }
    }

    pub const fn cache_ref(&'a self) -> &'a PersistentCache {
        self.cache
    }

    pub const fn into_cache(self) -> &'a PersistentCache {
        self.cache
    }

    /// Toggle the disabled-communication intersection. Defaults to enabled.
    pub const fn check_member_communication_disabled(
        mut self,
        check_member_communication_disabled: bool,
    ) -> Self {
        self.check_member_communication_disabled = check_member_communication_disabled;
        self
    }

    /// Permissions a member has in a specific guild channel.
    ///
    /// Returns [`Permissions::all`] if the user owns the guild. For threads,
    /// permission overwrites from the parent channel are merged in.
    pub fn in_channel(
        &self,
        user_id: Id<UserMarker>,
        channel_id: Id<ChannelMarker>,
    ) -> Result<Permissions, ChannelError> {
        let channel = self.cache.channels.get(&channel_id).ok_or_else(|| ChannelError {
            kind: ChannelErrorType::ChannelUnavailable { channel_id },
            source: None,
        })?;

        let guild_id = channel.guild_id.ok_or(ChannelError {
            kind: ChannelErrorType::ChannelNotInGuild { channel_id },
            source: None,
        })?;

        if self.is_owner(user_id, guild_id) {
            return Ok(Permissions::all());
        }

        let member = self.cache.members.get(&(guild_id, user_id)).ok_or(ChannelError {
            kind: ChannelErrorType::MemberUnavailable { guild_id, user_id },
            source: None,
        })?;

        let MemberRoles { assigned, everyone } = self
            .member_roles(guild_id, &member)
            .map_err(ChannelError::from_member_roles)?;

        let overwrites = match channel.kind {
            ChannelType::AnnouncementThread
            | ChannelType::PrivateThread
            | ChannelType::PublicThread => self.parent_overwrites(&channel)?,
            _ => channel.permission_overwrites.clone().unwrap_or_default(),
        };

        let calculator =
            PermissionCalculator::new(guild_id, user_id, everyone, assigned.as_slice());

        let permissions = calculator.in_channel(channel.kind, overwrites.as_slice());
        Ok(self.disable_member_communication(&member, permissions))
    }

    /// Guild-level permissions of a member.
    ///
    /// Returns [`Permissions::all`] for the guild owner.
    pub fn root(
        &self,
        user_id: Id<UserMarker>,
        guild_id: Id<GuildMarker>,
    ) -> Result<Permissions, RootError> {
        if self.is_owner(user_id, guild_id) {
            return Ok(Permissions::all());
        }

        let member = self.cache.members.get(&(guild_id, user_id)).ok_or(RootError {
            kind: RootErrorType::MemberUnavailable { guild_id, user_id },
            source: None,
        })?;

        let MemberRoles { assigned, everyone } = self
            .member_roles(guild_id, &member)
            .map_err(RootError::from_member_roles)?;

        let calculator =
            PermissionCalculator::new(guild_id, user_id, everyone, assigned.as_slice());

        let permissions = calculator.root();
        Ok(self.disable_member_communication(&member, permissions))
    }

    fn disable_member_communication(&self, member: &Member, permissions: Permissions) -> Permissions {
        // Admin overrides any timeout. The flag also lets callers opt out
        // entirely (e.g. for offline analysis where system time is suspect).
        if !self.check_member_communication_disabled
            || permissions.contains(Permissions::ADMINISTRATOR)
        {
            return permissions;
        }

        let Some(until) = member.communication_disabled_until else {
            return permissions;
        };

        let Ok(absolute): Result<u64, _> = until.as_micros().try_into() else {
            return permissions;
        };

        let ends = SystemTime::UNIX_EPOCH + Duration::from_micros(absolute);
        if SystemTime::now() > ends {
            return permissions;
        }

        permissions.intersection(MEMBER_COMMUNICATION_DISABLED_ALLOWLIST)
    }

    fn is_owner(&self, user_id: Id<UserMarker>, guild_id: Id<GuildMarker>) -> bool {
        self.cache
            .guilds
            .get(&guild_id)
            .is_some_and(|g| g.owner_id == user_id)
    }

    fn member_roles(
        &self,
        guild_id: Id<GuildMarker>,
        member: &Member,
    ) -> Result<MemberRoles, MemberRolesErrorType> {
        let mut assigned = Vec::with_capacity(member.roles.len());
        for role_id in &member.roles {
            let role = self
                .cache
                .roles
                .get(role_id)
                .ok_or(MemberRolesErrorType::RoleMissing { role_id: *role_id })?;
            assigned.push((*role_id, role.value.permissions));
        }

        let everyone_role_id = guild_id.cast();
        let everyone_role = self
            .cache
            .roles
            .get(&everyone_role_id)
            .ok_or(MemberRolesErrorType::RoleMissing { role_id: everyone_role_id })?;

        Ok(MemberRoles { assigned, everyone: everyone_role.value.permissions })
    }

    fn parent_overwrites(&self, thread: &Channel) -> Result<Vec<PermissionOverwrite>, ChannelError> {
        let parent_id = thread.parent_id.ok_or(ChannelError {
            kind: ChannelErrorType::ParentChannelNotPresent { thread_id: thread.id },
            source: None,
        })?;

        let parent = self.cache.channels.get(&parent_id).ok_or(ChannelError {
            kind: ChannelErrorType::ChannelUnavailable { channel_id: parent_id },
            source: None,
        })?;

        if parent.guild_id.is_none() {
            return Err(ChannelError {
                kind: ChannelErrorType::ChannelNotInGuild { channel_id: parent.id },
                source: None,
            });
        }

        let parent_overwrites = parent.permission_overwrites.as_deref().unwrap_or(&[]);
        let thread_overwrites = thread.permission_overwrites.as_deref().unwrap_or(&[]);

        let mut out = Vec::with_capacity(parent_overwrites.len() + thread_overwrites.len());
        out.extend_from_slice(parent_overwrites);
        out.extend_from_slice(thread_overwrites);
        Ok(out)
    }
}
