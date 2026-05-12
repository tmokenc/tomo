//! Permission lookup with cache-miss hydration.
//!
//! Wraps `tomo_cache::PersistentCachePermissions::root` so that missing
//! members / roles are fetched from Discord on the spot and re-cached,
//! avoiding false-negative denials on a cold cache.
//!
//! Two cache failure paths matter:
//!   * the invoking member isn't cached yet (gateway has not delivered
//!     `MEMBER_ADD` / `GUILD_CREATE` populated members yet);
//!   * one of the member's roles isn't cached yet (new role, or partial
//!     `GUILD_CREATE` delivery).
//!
//! For each, we issue the targeted HTTP call, `upsert_*` into the cache,
//! and retry. Only when *both* the targeted hydrate and the retry fail do
//! we surface an error to the caller.
//!
//! The retry loop is bounded — at most three rounds (one for member, up to
//! two for role chains since fetching one role can reveal a second
//! still-missing one). A pathological response can't pin a request here.

use tomo_cache::permission::RootErrorType;
use tomo_core::error::{Error, Result};
use twilight_model::guild::Permissions;
use twilight_model::id::Id;
use twilight_model::id::marker::{GuildMarker, UserMarker};

use crate::state::Bot;

/// Hard upper bound on hydrate retries. Prevents an infinite loop if
/// Discord returns the same `Unavailable` error after we just upserted the
/// requested resource (would indicate a serialisation / cache bug, not a
/// real upstream issue).
const MAX_HYDRATE_ATTEMPTS: usize = 3;

/// Resolve `user_id`'s effective guild-level permissions, hydrating the
/// cache from Discord HTTP if the calculator hits an `Unavailable` member
/// or role. Returns the computed [`Permissions`] on success.
pub async fn resolve(
    bot: &Bot,
    user_id: Id<UserMarker>,
    guild_id: Id<GuildMarker>,
) -> Result<Permissions> {
    for attempt in 0..MAX_HYDRATE_ATTEMPTS {
        match bot.cache.permissions().root(user_id, guild_id) {
            Ok(perms) => return Ok(perms),
            Err(e) => match e.kind() {
                RootErrorType::MemberUnavailable { .. } => {
                    hydrate_member(bot, guild_id, user_id).await?;
                }
                RootErrorType::RoleUnavailable { role_id } => {
                    let role_id = *role_id;
                    // `@everyone` shares the guild id. Discord doesn't expose
                    // it as a separate role to fetch; bulk-load every role
                    // in the guild instead so the `@everyone` slot lands
                    // along with whichever role triggered the error.
                    if role_id.cast::<GuildMarker>() == guild_id {
                        hydrate_roles(bot, guild_id).await?;
                    } else {
                        // Targeted fetch — also bulk so we don't loop
                        // hydrating individual roles one at a time.
                        hydrate_roles(bot, guild_id).await?;
                    }
                }
                other => {
                    return Err(Error::config(format!(
                        "permission calc non-hydratable error: {other:?}"
                    )));
                }
            },
        }
        // Loop iteration is upper-bounded by the constant above. Surfaces
        // a clear bail-out message rather than spinning forever.
        if attempt + 1 == MAX_HYDRATE_ATTEMPTS {
            return Err(Error::config(format!(
                "permission calc still failing after {MAX_HYDRATE_ATTEMPTS} hydrate attempts"
            )));
        }
    }
    Err(Error::config("permission calc: unreachable retry loop exit"))
}

/// `convenience` wrapper used by command implementations — returns `true`
/// if the user is a bot owner OR they hold every bit in `required` after
/// hydration. Owners bypass every server's checks, by design.
///
/// Returns `Err` only on irrecoverable HTTP / cache errors — callers can
/// treat that as "deny and log" if the perm is required to proceed.
pub async fn user_has_or_is_owner(
    bot: &Bot,
    user_id: Id<UserMarker>,
    guild_id: Id<GuildMarker>,
    required: Permissions,
) -> Result<bool> {
    if bot.is_owner(user_id) {
        return Ok(true);
    }
    let perms = resolve(bot, user_id, guild_id).await?;
    Ok(perms.contains(required))
}

async fn hydrate_member(
    bot: &Bot,
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
) -> Result<()> {
    let member = bot
        .http
        .guild_member(guild_id, user_id)
        .await
        .map_err(|e| Error::config(format!("fetch member {user_id} in guild {guild_id}: {e}")))?
        .model()
        .await
        .map_err(|e| Error::config(format!("decode member: {e}")))?;
    bot.cache.upsert_member(guild_id, member);
    Ok(())
}

async fn hydrate_roles(bot: &Bot, guild_id: Id<GuildMarker>) -> Result<()> {
    let roles = bot
        .http
        .roles(guild_id)
        .await
        .map_err(|e| Error::config(format!("fetch roles for guild {guild_id}: {e}")))?
        .model()
        .await
        .map_err(|e| Error::config(format!("decode roles: {e}")))?;
    for role in roles {
        bot.cache.upsert_role(guild_id, role);
    }
    Ok(())
}

