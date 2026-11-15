use std::collections::HashMap;
use std::path::Path;

use super::*;

#[test]
fn title_combines_folder_and_branch() {
    assert_eq!(
        auto_tab_title(Path::new("/work/MiCajaDigital"), Some("main")).as_deref(),
        Some("MiCajaDigital · main")
    );
}

#[test]
fn title_without_branch_is_folder_only() {
    assert_eq!(
        auto_tab_title(Path::new("/work/SIPE"), None).as_deref(),
        Some("SIPE")
    );
    assert_eq!(
        auto_tab_title(Path::new("/work/SIPE"), Some("  ")).as_deref(),
        Some("SIPE")
    );
}

#[test]
fn title_strips_control_characters_and_rejects_root() {
    assert_eq!(
        auto_tab_title(Path::new("/work/a\u{1b}b"), Some("x\ny")).as_deref(),
        Some("ab · xy")
    );
    assert_eq!(auto_tab_title(Path::new("/"), Some("main")), None);
}

#[test]
fn project_color_is_stable_and_from_palette() {
    let dir = Path::new("/work/MiCajaDigital");
    assert_eq!(project_color(dir), project_color(dir));
    assert!(TAB_COLOR_OPTIONS.contains(&project_color(dir)));
}

#[test]
fn pane_template_keeps_cwd_out_of_command() {
    let template = claude_pane_template(PathBuf::from("/work/a b; rm -rf"), "  claude  ");
    match template {
        PaneTemplateType::PaneTemplate { cwd, commands, .. } => {
            assert_eq!(cwd, PathBuf::from("/work/a b; rm -rf"));
            assert_eq!(commands, vec![CommandTemplate::from("claude")]);
        }
        PaneTemplateType::PaneBranchTemplate { .. } => panic!("expected a single pane"),
    }
}

#[test]
fn blank_command_opens_plain_shell() {
    match claude_pane_template(PathBuf::from("/x"), "  ") {
        PaneTemplateType::PaneTemplate { commands, .. } => assert!(commands.is_empty()),
        PaneTemplateType::PaneBranchTemplate { .. } => panic!("expected a single pane"),
    }
}

#[test]
fn workspaces_become_sorted_launch_configs_skipping_missing_dirs() {
    let workspaces = HashMap::from([
        (
            "Zeta".to_string(),
            vec!["/a".to_string(), "/missing".to_string()],
        ),
        ("Alfa".to_string(), vec!["/b".to_string()]),
        ("Vacio".to_string(), vec!["/missing".to_string()]),
        (" ".to_string(), vec!["/a".to_string()]),
    ]);
    let configs =
        workspace_launch_configs(&workspaces, "claude", |dir| dir != Path::new("/missing"));

    let names: Vec<_> = configs.iter().map(|config| config.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Abrir espacio de trabajo: Alfa",
            "Abrir espacio de trabajo: Zeta"
        ]
    );
    assert_eq!(configs[1].windows[0].tabs.len(), 1);
}
