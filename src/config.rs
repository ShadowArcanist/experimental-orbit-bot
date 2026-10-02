//! Runtime configuration read from environment variables.

use std::collections::HashSet;

use anyhow::{Context as _, bail};
use chrono_tz::Tz;
use poise::serenity_prelude::UserId;

/// Configuration for the bot.
pub struct Config {
    /// The Discord bot token.
    pub discord_token: String,
    /// Discord user IDs that may use the bot. Everyone else is refused.
    pub allowed_users: HashSet<UserId>,
    /// Orbit connection settings.
    pub orbit: OrbitConfig,
    /// Time zone used to read due dates such as `tomorrow` or `2026-10-05`.
    pub timezone: Tz,
    /// Project key used when a command does not name a project.
    pub default_project: Option<String>,
}

/// How to reach and sign in to Orbit.
#[derive(Clone)]
pub struct OrbitConfig {
    /// Public origin of Orbit, e.g. `https://orbit.example.com` (no trailing slash).
    pub url: String,
    /// Email of the Orbit bot user.
    pub email: String,
    /// Password of the Orbit bot user.
    pub password: String,
    /// Workspace to use. When unset, the bot uses the only workspace it can see.
    pub workspace_id: Option<String>,
}

impl Config {
    /// Read the configuration from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        let timezone = match optional("TIMEZONE") {
            Some(name) => name
                .parse::<Tz>()
                .map_err(|_| anyhow::anyhow!("TIMEZONE `{name}` is not an IANA time zone"))?,
            None => Tz::UTC,
        };
        Ok(Self {
            discord_token: required("DISCORD_TOKEN")?,
            allowed_users: parse_user_ids(&required("ALLOWED_USER_IDS")?)?,
            orbit: OrbitConfig::from_env()?,
            timezone,
            default_project: optional("ORBIT_DEFAULT_PROJECT"),
        })
    }
}

impl OrbitConfig {
    /// Read only the Orbit settings (used by `orbit-bot check`).
    pub fn from_env() -> anyhow::Result<Self> {
        let url = required("ORBIT_URL")?.trim_end_matches('/').to_owned();
        let parsed = reqwest::Url::parse(&url).context("ORBIT_URL must be a valid URL")?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || parsed.path() != "/"
            || parsed.query().is_some()
        {
            bail!("ORBIT_URL must be an origin such as `https://orbit.example.com` (no path)");
        }
        Ok(Self {
            url,
            email: required("ORBIT_EMAIL")?,
            password: required("ORBIT_PASSWORD")?,
            workspace_id: optional("ORBIT_WORKSPACE_ID"),
        })
    }
}

/// Parse a comma- or space-separated list of Discord user IDs.
pub fn parse_user_ids(raw: &str) -> anyhow::Result<HashSet<UserId>> {
    let ids = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| match part.parse::<u64>() {
            Ok(id) if id != 0 => Ok(UserId::new(id)),
            _ => bail!("ALLOWED_USER_IDS contains `{part}`, which is not a Discord user ID"),
        })
        .collect::<anyhow::Result<HashSet<_>>>()?;
    if ids.is_empty() {
        bail!("ALLOWED_USER_IDS must list at least one Discord user ID");
    }
    Ok(ids)
}

fn required(name: &str) -> anyhow::Result<String> {
    optional(name)
        .with_context(|| format!("{name} is not set (put it in your environment or .env)"))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_id_lists() {
        let ids = parse_user_ids(" 123, 456 789 ").unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&UserId::new(456)));
    }

    #[test]
    fn rejects_bad_user_ids() {
        assert!(parse_user_ids("").is_err());
        assert!(parse_user_ids("123,abc").is_err());
        assert!(parse_user_ids("0").is_err());
    }
}
