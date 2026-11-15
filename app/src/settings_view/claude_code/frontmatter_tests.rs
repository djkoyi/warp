use super::*;

#[test]
fn parses_frontmatter_and_body() {
    let doc = parse_markdown("---\nname: deploy\ndescription: Ship it\n---\n\n# Steps\nRun.\n")
        .unwrap();
    assert_eq!(doc.get("name").as_deref(), Some("deploy"));
    assert_eq!(doc.get("description").as_deref(), Some("Ship it"));
    assert_eq!(doc.body, "\n# Steps\nRun.\n");
}

#[test]
fn missing_frontmatter_is_all_body() {
    let doc = parse_markdown("just text\n").unwrap();
    assert!(doc.frontmatter.is_empty());
    assert_eq!(doc.body, "just text\n");
}

#[test]
fn crlf_and_bom_are_tolerated() {
    let doc = parse_markdown("\u{feff}---\r\nname: x\r\n---\r\nbody\r\n").unwrap();
    assert_eq!(doc.get("name").as_deref(), Some("x"));
    assert_eq!(doc.body, "body\r\n");
}

#[test]
fn invalid_yaml_is_an_error() {
    assert!(parse_markdown("---\nname: [unclosed\n---\nbody\n").is_err());
    assert!(parse_markdown("---\n- a\n- b\n---\nbody\n").is_err());
}

#[test]
fn round_trip_preserves_unknown_keys_and_order() {
    let mut doc = parse_markdown(
        "---\nname: reviewer\ntools:\n  - Read\n  - Grep\nmodel: sonnet\ndescription: Old\n---\nBody\n",
    )
    .unwrap();
    doc.set_string("description", "New description");
    doc.set_string_if_changed("tools", "Read, Grep");
    doc.set_string("model", "");
    let rendered = render_markdown(&doc).unwrap();
    let reparsed = parse_markdown(&rendered).unwrap();
    let keys: Vec<_> = reparsed
        .frontmatter
        .iter()
        .map(|(k, _)| k.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, vec!["name", "tools", "description"]);
    assert!(matches!(
        reparsed.frontmatter.get(&Value::String("tools".into())),
        Some(Value::Sequence(_))
    ));
    assert_eq!(reparsed.get("description").as_deref(), Some("New description"));
    assert_eq!(reparsed.body, "\nBody\n");
    assert!(rendered.starts_with("---\nname: reviewer\n"));
}

#[test]
fn render_with_empty_frontmatter() {
    let doc = MarkdownDoc {
        frontmatter: Mapping::new(),
        body: "Hello".to_owned(),
    };
    assert_eq!(render_markdown(&doc).unwrap(), "---\n---\n\nHello\n");
}
