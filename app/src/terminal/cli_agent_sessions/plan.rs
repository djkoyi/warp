//! The Claude subscription plan of the signed-in account (e.g. "Ultra", "Max 20x", "Pro"), read
//! from the `oauthAccount` object Claude Code keeps in its own `.claude.json`. Read-only: nothing
//! here writes to that file or touches credentials.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde_json::Value;

use crate::settings_view::claude_code::paths::ClaudePaths;

/// How often the plan file is checked for changes. The plan only changes when the user upgrades,
/// downgrades or signs in with another account.
const PLAN_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Longest plan name shown in the top bar.
const MAX_PLAN_NAME_CHARS: usize = 24;

/// The subscription plan of the signed-in Claude account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePlan {
    /// The plan as the account reports it, e.g. `Ultra`, or the tier name when it reports none.
    pub name: String,
    /// The usage tier when it adds information to the name, e.g. `Max 20x`.
    pub tier: Option<String>,
}

impl ClaudePlan {
    /// `Plan Ultra`.
    pub fn label(&self) -> String {
        format!("Plan {}", self.name)
    }

    /// `Plan Ultra · Max 20x`, or just the label when the tier adds nothing.
    pub fn full_label(&self) -> String {
        match &self.tier {
            Some(tier) => format!("{} · {tier}", self.label()),
            None => self.label(),
        }
    }
}

/// Reads the plan from the parsed `.claude.json`. `None` when the account is not a subscription
/// (API key, Bedrock, Vertex) or the file carries no recognizable plan.
pub fn parse_plan(root: &Value) -> Option<ClaudePlan> {
    let account = root.get("oauthAccount")?;
    let text = |key: &str| {
        account
            .get(key)
            .and_then(Value::as_str)
            .map(clean_name)
            .filter(|value| !value.is_empty())
    };
    let tier = tier_name(
        account.get("organizationType").and_then(Value::as_str),
        account
            .get("organizationRateLimitTier")
            .and_then(Value::as_str),
    );
    match (text("planDisplayName"), tier) {
        (Some(name), tier) => {
            let tier = tier.filter(|tier| !tier.eq_ignore_ascii_case(&name));
            Some(ClaudePlan { name, tier })
        }
        (None, Some(tier)) => Some(ClaudePlan {
            name: tier,
            tier: None,
        }),
        (None, None) => None,
    }
}

/// Human name of the usage tier: `Max 20x`, `Max 5x`, `Pro`, `Team`, `Enterprise`.
fn tier_name(organization_type: Option<&str>, rate_limit_tier: Option<&str>) -> Option<String> {
    let base = match organization_type? {
        "claude_max" => "Max",
        "claude_pro" => "Pro",
        "claude_team" => "Team",
        "claude_enterprise" => "Enterprise",
        _ => return None,
    };
    let multiplier = rate_limit_tier.and_then(|tier| {
        tier.rsplit('_').find(|part| {
            part.strip_suffix('x').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
    });
    Some(match multiplier {
        Some(multiplier) => format!("{base} {multiplier}"),
        None => base.to_owned(),
    })
}

/// Trims, drops control characters and caps the length of a name that comes from a file.
fn clean_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PLAN_NAME_CHARS)
        .collect()
}

fn read_plan(path: &Path) -> Option<ClaudePlan> {
    let bytes = std::fs::read(path).ok()?;
    parse_plan(&serde_json::from_slice(&bytes).ok()?)
}

struct CachedPlan {
    checked_at: Instant,
    modified: Option<SystemTime>,
    plan: Option<ClaudePlan>,
}

static CACHE: Mutex<Option<CachedPlan>> = Mutex::new(None);

/// The plan of the signed-in account. Cheap to call from rendering code: the file is only
/// re-read when it changed on disk, and that is checked at most once per
/// [`PLAN_RECHECK_INTERVAL`].
pub fn current_plan() -> Option<ClaudePlan> {
    let mut cache = CACHE.lock();
    if let Some(cached) = cache.as_ref()
        && cached.checked_at.elapsed() < PLAN_RECHECK_INTERVAL
    {
        return cached.plan.clone();
    }
    let path = ClaudePaths::from_env(None).map(|paths| paths.claude_json);
    let modified = path
        .as_deref()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok());
    let plan = match cache.take() {
        Some(cached) if cached.modified == modified && modified.is_some() => cached.plan,
        _ => path.as_deref().and_then(read_plan),
    };
    *cache = Some(CachedPlan {
        checked_at: Instant::now(),
        modified,
        plan: plan.clone(),
    });
    plan
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
