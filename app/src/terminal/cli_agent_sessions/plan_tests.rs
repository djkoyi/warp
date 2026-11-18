use serde_json::json;

use super::*;

fn plan(name: &str, tier: Option<&str>) -> ClaudePlan {
    ClaudePlan {
        name: name.to_owned(),
        tier: tier.map(str::to_owned),
    }
}

#[test]
fn parse_plan_prefers_the_reported_display_name() {
    let root = json!({
        "oauthAccount": {
            "planDisplayName": "Ultra",
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
        }
    });
    let parsed = parse_plan(&root).unwrap();
    assert_eq!(parsed, plan("Ultra", Some("Max 20x")));
    assert_eq!(parsed.label(), "Plan Ultra");
    assert_eq!(parsed.full_label(), "Plan Ultra · Max 20x");
}

#[test]
fn parse_plan_falls_back_to_the_tier_name() {
    let max = json!({
        "oauthAccount": {
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_5x",
        }
    });
    assert_eq!(parse_plan(&max), Some(plan("Max 5x", None)));

    let pro = json!({ "oauthAccount": { "organizationType": "claude_pro" } });
    let parsed = parse_plan(&pro).unwrap();
    assert_eq!(parsed, plan("Pro", None));
    assert_eq!(parsed.full_label(), "Plan Pro");
}

#[test]
fn parse_plan_drops_a_tier_that_repeats_the_name() {
    let root = json!({
        "oauthAccount": { "planDisplayName": "pro", "organizationType": "claude_pro" }
    });
    assert_eq!(parse_plan(&root), Some(plan("pro", None)));
}

#[test]
fn parse_plan_is_none_without_a_subscription() {
    assert_eq!(parse_plan(&json!({})), None);
    assert_eq!(parse_plan(&json!({ "oauthAccount": {} })), None);
    assert_eq!(
        parse_plan(&json!({ "oauthAccount": { "organizationType": "api", "planDisplayName": "  " } })),
        None
    );
}

#[test]
fn parse_plan_cleans_names_read_from_the_file() {
    let root = json!({
        "oauthAccount": { "planDisplayName": "  Ul\u{1b}tra\n  " }
    });
    assert_eq!(parse_plan(&root), Some(plan("Ultra", None)));

    let long = "x".repeat(80);
    let root = json!({ "oauthAccount": { "planDisplayName": long } });
    assert_eq!(parse_plan(&root).unwrap().name.chars().count(), 24);
}
