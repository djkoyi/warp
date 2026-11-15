use std::path::Path;

use super::*;

#[test]
fn claude_command_quotes_hostile_prompt() {
    let command = claude_task_command(
        Path::new("/Users/fr/Mis Proyectos/sipe"),
        "Revisa $HOME; rm -rf / && echo `id` 'citado' \"doble\" !!",
    );
    assert_eq!(
        command,
        "cd -- '/Users/fr/Mis Proyectos/sipe' && claude --dangerously-skip-permissions -p -- \
         'Revisa $HOME; rm -rf / && echo `id` '\\''citado'\\'' \"doble\" !!'"
    );
    let words = shell_words::split(&command).unwrap();
    assert_eq!(
        words.last().unwrap(),
        "Revisa $HOME; rm -rf / && echo `id` 'citado' \"doble\" !!"
    );
}

#[test]
fn claude_command_flattens_newlines() {
    let command = claude_task_command(Path::new("/tmp"), "línea uno\n\n  línea dos\r\n");
    assert!(!command.contains('\n'));
    assert!(command.ends_with("-p -- 'línea uno línea dos'"));
}

#[test]
fn claude_command_keeps_backslashes_outside_single_quotes() {
    // fish reads `\\` inside single quotes as one backslash, so they must not appear there.
    let command = claude_task_command(Path::new("/tmp"), r"C:\dir\");
    assert!(command.ends_with(r"-p -- 'C:'\\'dir'\\''"));
    let words = shell_words::split(&command).unwrap();
    assert_eq!(words.last().unwrap(), r"C:\dir\");
}

#[test]
fn claude_command_ends_options_before_a_dash_prompt() {
    let command = claude_task_command(Path::new("/tmp"), "--help me");
    assert!(command.ends_with("-p -- '--help me'"));
}

#[test]
fn preview_command_quotes_only_the_dir() {
    assert_eq!(
        preview_command(Path::new("/tmp/a b"), "  npm run dev "),
        "cd -- '/tmp/a b' && npm run dev"
    );
}

#[test]
fn expand_dir_expands_tilde() {
    let home = dirs::home_dir().unwrap();
    assert_eq!(expand_dir("~/x"), home.join("x"));
    assert_eq!(expand_dir("/abs"), Path::new("/abs"));
}
