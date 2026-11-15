use std::path::Path;

use super::*;

#[test]
fn extracts_vite_next_and_bare_urls() {
    let text = "\
  VITE v5.0.0  ready in 300 ms

  ➜  Local:   http://localhost:5173/
  ➜  Network: use --host to expose
   ▲ Next.js 14
   - Local:        http://127.0.0.1:3000.
Server listening on 0.0.0.0:8080, ready
Again: http://localhost:5173/
";
    assert_eq!(
        extract_dev_server_urls(text),
        vec![
            "http://localhost:5173/",
            "http://127.0.0.1:3000",
            "http://localhost:8080",
        ]
    );
}

#[test]
fn keeps_https_and_paths() {
    assert_eq!(
        extract_dev_server_urls("open https://LOCALHOST:8443/app/index.html, please"),
        vec!["https://localhost:8443/app/index.html"]
    );
}

#[test]
fn ignores_database_urls_and_ports() {
    let text = "DATABASE_URL=postgres://user@localhost:5432/db\n\
                redis at localhost:6379\n\
                example.localhost:3000\n";
    assert!(extract_dev_server_urls(text).is_empty());
}

#[test]
fn explicit_http_on_db_port_is_kept() {
    assert_eq!(
        extract_dev_server_urls("http://localhost:5432"),
        vec!["http://localhost:5432"]
    );
}

#[test]
fn port_must_end_at_a_word_boundary() {
    assert!(extract_dev_server_urls("localhost:123456 and localhost:3000abc").is_empty());
}

#[test]
fn origin_strips_path() {
    assert_eq!(
        url_origin("http://localhost:5173/a/b"),
        "http://localhost:5173"
    );
    assert_eq!(url_origin("http://localhost:5173"), "http://localhost:5173");
}

#[test]
fn package_json_prefers_dev_script() {
    assert_eq!(
        package_json_dev_script(r#"{"scripts":{"start":"node .","dev":"vite"}}"#),
        Some("dev")
    );
    assert_eq!(
        package_json_dev_script(r#"{"scripts":{"start":"node ."}}"#),
        Some("start")
    );
    assert_eq!(package_json_dev_script(r#"{"name":"x"}"#), None);
    assert_eq!(package_json_dev_script("not json"), None);
}

#[test]
fn configured_command_deepest_match_wins() {
    let configured = vec![
        PreviewCommand {
            project_dir: "/work".to_owned(),
            command: "make serve".to_owned(),
        },
        PreviewCommand {
            project_dir: "/work/app".to_owned(),
            command: "npm run dev".to_owned(),
        },
    ];
    assert_eq!(
        resolve_preview_command(&configured, Path::new("/work/app/src")),
        Some((PathBuf::from("/work/app"), "npm run dev".to_owned()))
    );
    assert_eq!(
        resolve_preview_command(&configured, Path::new("/work/other")),
        Some((PathBuf::from("/work"), "make serve".to_owned()))
    );
}

#[test]
fn falls_back_to_package_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
    let nested = dir.path().join("src/components");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        resolve_preview_command(&[], &nested),
        Some((dir.path().to_path_buf(), "pnpm dev".to_owned()))
    );
}
