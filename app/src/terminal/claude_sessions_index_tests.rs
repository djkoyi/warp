use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::json;

use super::{
    HEAD_BYTES, MAX_TITLE_CHARS, TAIL_BYTES, display_cwd, read_session, recent_sessions,
    relative_time_es, title_from_prompt,
};

const ID_A: &str = "0f6c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
const ID_B: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
const ID_C: &str = "2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e";

fn user_line(cwd: &str, content: serde_json::Value) -> String {
    json!({
        "type": "user",
        "cwd": cwd,
        "isSidechain": false,
        "message": { "role": "user", "content": content },
    })
    .to_string()
}

fn write_transcript(dir: &Path, project: &str, id: &str, lines: &[String]) -> PathBuf {
    let project_dir = dir.join(project);
    fs::create_dir_all(&project_dir).unwrap();
    let path = project_dir.join(format!("{id}.jsonl"));
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[test]
fn first_user_message_is_the_title_when_there_is_no_saved_title() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_transcript(
        dir.path(),
        "-tmp-demo",
        ID_A,
        &[
            json!({"type": "permission-mode", "permissionMode": "default"}).to_string(),
            json!({"type": "attachment", "cwd": "/tmp/demo"}).to_string(),
            user_line("/tmp/demo", json!("<command-name>/clear</command-name>")),
            json!({"type": "user", "cwd": "/tmp/demo", "isMeta": true,
                   "message": {"content": "meta"}})
            .to_string(),
            user_line(
                "/tmp/demo",
                json!([{"type": "text", "text": "  Arregla el\nmenu de pestanas  "}]),
            ),
            user_line("/tmp/demo", json!("segundo mensaje")),
        ],
    );

    let entry = read_session(&path, SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(entry.session_id, ID_A);
    assert_eq!(entry.title, "Arregla el menu de pestanas");
    assert_eq!(entry.cwd, "/tmp/demo");
}

#[test]
fn latest_saved_title_wins_and_custom_title_beats_generated_one() {
    let dir = tempfile::tempdir().unwrap();
    let generated = write_transcript(
        dir.path(),
        "-tmp-demo",
        ID_A,
        &[
            user_line("/tmp/demo", json!("hola")),
            json!({"type": "ai-title", "aiTitle": "Titulo viejo"}).to_string(),
            json!({"type": "ai-title", "aiTitle": "Titulo nuevo"}).to_string(),
        ],
    );
    assert_eq!(
        read_session(&generated, SystemTime::UNIX_EPOCH)
            .unwrap()
            .title,
        "Titulo nuevo"
    );

    let custom = write_transcript(
        dir.path(),
        "-tmp-demo",
        ID_B,
        &[
            user_line("/tmp/demo", json!("hola")),
            json!({"type": "custom-title", "customTitle": "Mi nombre"}).to_string(),
            json!({"type": "ai-title", "aiTitle": "Generado despues"}).to_string(),
        ],
    );
    assert_eq!(
        read_session(&custom, SystemTime::UNIX_EPOCH).unwrap().title,
        "Mi nombre"
    );

    let summary = write_transcript(
        dir.path(),
        "-tmp-demo",
        ID_C,
        &[
            json!({"type": "summary", "summary": "Resumen antiguo"}).to_string(),
            user_line("/tmp/demo", json!("hola")),
        ],
    );
    assert_eq!(
        read_session(&summary, SystemTime::UNIX_EPOCH)
            .unwrap()
            .title,
        "Resumen antiguo"
    );
}

#[test]
fn transcripts_without_user_messages_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let only_meta = write_transcript(
        dir.path(),
        "-tmp-demo",
        ID_A,
        &[
            json!({"type": "permission-mode", "permissionMode": "default"}).to_string(),
            user_line(
                "/tmp/demo",
                json!("<local-command-stdout></local-command-stdout>"),
            ),
            json!({"type": "user", "cwd": "/tmp/demo",
                   "message": {"content": [{"type": "tool_result", "content": "x"}]}})
            .to_string(),
        ],
    );
    assert_eq!(read_session(&only_meta, SystemTime::UNIX_EPOCH), None);

    let garbage = write_transcript(dir.path(), "-tmp-demo", ID_B, &["no es json".to_string()]);
    assert_eq!(read_session(&garbage, SystemTime::UNIX_EPOCH), None);
}

#[test]
fn title_is_found_past_a_large_middle_and_cwd_past_large_attachments() {
    let dir = tempfile::tempdir().unwrap();
    let filler = "x".repeat(64 * 1024);
    let mut lines = vec![json!({"type": "attachment", "content": filler}).to_string(); 4];
    lines.push(user_line("/tmp/grande", json!("primer mensaje")));
    let middle_lines = (HEAD_BYTES + TAIL_BYTES) as usize / filler.len() + 2;
    lines.extend(std::iter::repeat_n(
        json!({"type": "assistant", "content": filler}).to_string(),
        middle_lines,
    ));
    lines.push(json!({"type": "ai-title", "aiTitle": "Titulo al final"}).to_string());
    let path = write_transcript(dir.path(), "-tmp-grande", ID_A, &lines);

    let entry = read_session(&path, SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(entry.title, "Titulo al final");
    assert_eq!(entry.cwd, "/tmp/grande");
}

#[test]
fn recent_sessions_are_newest_first_limited_and_ignore_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let old = write_transcript(
        dir.path(),
        "-tmp-a",
        ID_A,
        &[user_line("/tmp/a", json!("a"))],
    );
    let new = write_transcript(
        dir.path(),
        "-tmp-b",
        ID_B,
        &[user_line("/tmp/b", json!("b"))],
    );
    let mid = write_transcript(
        dir.path(),
        "-tmp-a",
        ID_C,
        &[user_line("/tmp/a", json!("c"))],
    );
    set_modified(&old, now - Duration::from_secs(300));
    set_modified(&new, now - Duration::from_secs(10));
    set_modified(&mid, now - Duration::from_secs(100));

    // Not transcripts of a resumable session.
    write_transcript(
        dir.path(),
        "-tmp-a",
        "agent-1234",
        &[user_line("/tmp/a", json!("x"))],
    );
    fs::write(dir.path().join("-tmp-a").join("notas.txt"), "x").unwrap();
    fs::write(
        dir.path().join("suelto.jsonl"),
        user_line("/tmp", json!("x")),
    )
    .unwrap();
    fs::write(dir.path().join("-tmp-a").join(format!("{ID_A}x.jsonl")), "").unwrap();

    let ids: Vec<String> = recent_sessions(dir.path(), 30)
        .into_iter()
        .map(|entry| entry.session_id)
        .collect();
    assert_eq!(ids, [ID_B, ID_C, ID_A]);

    let limited = recent_sessions(dir.path(), 2);
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].session_id, ID_B);
}

#[test]
fn resume_all_selects_newest_resumable_sessions_within_limit() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let newest = write_transcript(
        dir.path(),
        "-tmp-a",
        ID_A,
        &[user_line("/tmp/a", json!("a"))],
    );
    let middle = write_transcript(
        dir.path(),
        "-tmp-b",
        ID_B,
        &[user_line("/tmp/b", json!("b"))],
    );
    let oldest = write_transcript(
        dir.path(),
        "-tmp-c",
        ID_C,
        &[user_line("/tmp/c", json!("c"))],
    );
    set_modified(&newest, now - Duration::from_secs(10));
    set_modified(&middle, now - Duration::from_secs(100));
    set_modified(&oldest, now - Duration::from_secs(1000));

    // A transcript with no user message is not resumable and must never be opened.
    let empty = write_transcript(
        dir.path(),
        "-tmp-b",
        "3c4d5e6f-7a8b-4c9d-0e1f-2a3b4c5d6e7f",
        &[json!({"type": "permission-mode", "permissionMode": "default"}).to_string()],
    );
    set_modified(&empty, now - Duration::from_secs(1));

    let ids: Vec<String> = recent_sessions(dir.path(), 15)
        .into_iter()
        .map(|entry| entry.session_id)
        .collect();
    assert_eq!(ids, [ID_A, ID_B, ID_C]);

    let limited: Vec<String> = recent_sessions(dir.path(), 2)
        .into_iter()
        .map(|entry| entry.session_id)
        .collect();
    assert_eq!(limited, [ID_A, ID_B]);
}

#[test]
fn missing_projects_dir_yields_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(recent_sessions(&dir.path().join("no-existe"), 30).is_empty());
}

#[test]
fn long_prompts_are_cut_to_the_title_length() {
    let title = title_from_prompt(&"palabra ".repeat(40));
    assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    assert!(title.ends_with('…'));
    assert_eq!(title_from_prompt("corto"), "corto");
}

#[test]
fn cwd_under_home_is_shown_with_tilde() {
    let home = Path::new("/Users/demo");
    assert_eq!(
        display_cwd("/Users/demo/proyecto", Some(home)),
        "~/proyecto"
    );
    assert_eq!(display_cwd("/Users/demo", Some(home)), "~");
    assert_eq!(display_cwd("/Users/demo2/x", Some(home)), "/Users/demo2/x");
    assert_eq!(display_cwd("/opt/x", None), "/opt/x");
}

#[test]
fn relative_time_is_spanish() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 24 * 3600);
    let ago = |secs: u64| relative_time_es(now, now - Duration::from_secs(secs));
    assert_eq!(ago(5), "ahora mismo");
    assert_eq!(ago(5 * 60), "hace 5 min");
    assert_eq!(ago(3 * 3600), "hace 3 h");
    assert_eq!(ago(30 * 3600), "ayer");
    assert_eq!(ago(3 * 24 * 3600), "hace 3 días");
    assert_eq!(ago(7 * 24 * 3600), "hace 1 semana");
    assert_eq!(ago(15 * 24 * 3600), "hace 2 semanas");
    assert_eq!(ago(31 * 24 * 3600), "hace 1 mes");
    assert_eq!(ago(65 * 24 * 3600), "hace 2 meses");
    // A clock that moved backwards must not panic.
    assert_eq!(
        relative_time_es(now, now + Duration::from_secs(60)),
        "ahora mismo"
    );
}
