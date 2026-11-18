use super::*;

const PUBLISHED: &str = r#"{
  "producto": "WarpOss",
  "version": "1.2.0",
  "fecha": "2026-11-18",
  "plataformas": { "mac": { "url": "https://isoft.css.gob.pa/descargas/WarpOss.dmg" } },
  "pagina": "https://isoft.css.gob.pa/warposs/",
  "notas": "Dictado por voz corregido,\n  menu limpio."
}"#;

#[test]
fn newer_published_version_is_an_update() {
    let update = parse_update(PUBLISHED, "1.1.0").unwrap();
    assert_eq!(update.version, "1.2.0");
    assert_eq!(update.page_url, "https://isoft.css.gob.pa/warposs/");
    assert_eq!(
        update.notes.as_deref(),
        Some("Dictado por voz corregido, menu limpio.")
    );
    assert_eq!(update.label(), "Actualización 1.2.0");
    assert_eq!(
        update.tooltip_lines("1.1.0"),
        vec![
            "Hay una versión nueva de WarpOss: 1.2.0 (tienes la 1.1.0).".to_owned(),
            "Dictado por voz corregido, menu limpio.".to_owned(),
            "Clic para abrir la página de descarga.".to_owned(),
        ]
    );
}

#[test]
fn same_or_older_version_is_not_an_update() {
    assert_eq!(parse_update(PUBLISHED, "1.2.0"), None);
    assert_eq!(parse_update(PUBLISHED, "1.10.0"), None);
    assert_eq!(parse_update(PUBLISHED, "2.0.0"), None);
}

#[test]
fn versions_compare_numerically_not_as_text() {
    let manifest = r#"{ "version": "1.10.0" }"#;
    assert_eq!(parse_update(manifest, "1.9.0").unwrap().version, "1.10.0");
}

#[test]
fn untrusted_or_missing_page_falls_back_to_the_official_one() {
    for page in [
        r#""pagina": "https://evil.example/warposs/","#,
        r#""pagina": "http://isoft.css.gob.pa/warposs/","#,
        r#""pagina": "https://isoft.css.gob.pa.evil.example/","#,
        r#""pagina": "javascript:alert(1)","#,
        "",
    ] {
        let manifest = format!(r#"{{ {page} "version": "9.0.0" }}"#);
        assert_eq!(
            parse_update(&manifest, "1.1.0").unwrap().page_url,
            "https://isoft.css.gob.pa/warposs/",
            "{page}"
        );
    }
}

#[test]
fn malformed_manifests_are_ignored() {
    for manifest in [
        "",
        "<html>login</html>",
        "{}",
        r#"{ "version": "" }"#,
        r#"{ "version": 3 }"#,
        r#"{ "version": "1.2.0; rm -rf" }"#,
        r#"{ "version": "111111111111111111111111111111111111111" }"#,
    ] {
        assert_eq!(parse_update(manifest, "1.1.0"), None, "{manifest}");
    }
}

#[test]
fn current_version_is_a_plain_version() {
    let current = current_version();
    assert!(!current.is_empty());
    assert!(Version::from(current).is_some());
    assert!(current.chars().all(|c| c.is_ascii_digit() || c == '.'));
}
