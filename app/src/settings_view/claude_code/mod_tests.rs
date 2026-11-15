use super::*;

#[test]
fn validate_name_accepts_safe_identifiers() {
    assert!(validate_name("my-skill_2").is_ok());
    assert!(validate_name("A").is_ok());
}

#[test]
fn validate_name_rejects_unsafe_identifiers() {
    assert!(validate_name("").is_err());
    assert!(validate_name("../etc").is_err());
    assert!(validate_name("with space").is_err());
    assert!(validate_name("a/b").is_err());
    assert!(validate_name("ñandú").is_err());
    assert!(validate_name(&"x".repeat(65)).is_err());
}

#[test]
fn scopes_are_ordered_by_precedence() {
    assert!(SettingsScope::User < SettingsScope::Project);
    assert!(SettingsScope::Project < SettingsScope::Local);
    assert!(SettingsScope::Local < SettingsScope::Managed);
    assert!(!SettingsScope::Managed.is_writable());
}
