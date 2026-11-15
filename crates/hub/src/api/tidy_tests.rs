use super::*;

const BRAIN: &str = "/data/blazar/profiles/default/brain";

fn row(id: &str, node: &str, path: &str, parent: Option<&str>, project: Option<&str>) -> Item {
    Item {
        id: id.into(),
        node: node.into(),
        path: path.into(),
        name: id.into(),
        parent: parent.map(str::to_owned),
        project: project.map(str::to_owned),
        kind: "project".into(),
        running: false,
    }
}

fn shaped() -> Vec<Item> {
    let acme = Some("acme");
    vec![
        row("repo", "box-1", "/mnt/share/me/cad-repo", None, acme),
        row("cam", "box-1", "/mnt/share/me/cad-repo/cam", None, acme),
        row(
            "selfcad",
            "box-1",
            "/mnt/share/me/cad-repo/selfcad",
            None,
            acme,
        ),
        row(
            "furniture",
            "box-1",
            "/mnt/share/me/cad-repo/selfcad/sessions/furniture",
            None,
            acme,
        ),
        row(
            "drawer",
            "box-1",
            "/mnt/share/me/cad-repo/selfcad/sessions/drawer",
            Some("selfcad"),
            acme,
        ),
        row("loose", "box-1", "/mnt/share/me/cad-repo/notes", None, None),
        row("eval", "box-2", "/home/me/eval", Some("other"), None),
        row(
            "render",
            "box-2",
            "/home/me/eval/work/render",
            None,
            Some("pipeline"),
        ),
        row("rootdir", "box-3", "/root", None, None),
        row("deploy", "box-3", "/root/deploy", Some("rootdir"), None),
        row("plain", "box-3", "/root/plain", None, None),
        row(
            "peek",
            "local",
            &format!("{BRAIN}/s1/visual-check"),
            Some("repo"),
            acme,
        ),
        row("forge-a", "box-4", "/home/me/workspace/Forge", None, None),
        row("forge-b", "box-5", "/home/me/workspace/Forge", None, None),
        row(
            "demo-a",
            "local",
            "/Users/me/workspace/demo",
            None,
            Some("demo"),
        ),
        row("demo-b", "box-4", "/home/me/workspace/demo", None, None),
        row("ws-a", "box-4", "/home/me/workspace", None, None),
        row("ws-b", "box-5", "/srv/workspace", None, None),
    ]
}

fn nests(steps: &[Step]) -> Vec<(String, String)> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::Nest { id, under, .. } => Some((id.clone(), under.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_directory_tree_folds_under_its_deepest_workspace() {
    let steps = plan(&shaped(), BRAIN);
    let got = nests(&steps);
    for pair in [
        ("cam", "repo"),
        ("selfcad", "repo"),
        ("furniture", "selfcad"),
        ("loose", "repo"),
    ] {
        assert!(
            got.contains(&(pair.0.into(), pair.1.into())),
            "{pair:?} in {got:?}"
        );
    }
    assert!(
        !got.iter().any(|(id, _)| id == "drawer"),
        "already in place: {got:?}"
    );
    let loose = steps.iter().find_map(|s| match s {
        Step::Nest { id, project, .. } if id == "loose" => Some(project.clone()),
        _ => None,
    });
    assert_eq!(loose, Some(Some("acme".into())));
}

#[test]
fn a_different_project_or_a_root_folder_keeps_things_where_they_are() {
    let got = nests(&plan(&shaped(), BRAIN));
    assert!(!got.iter().any(|(id, _)| id == "render"), "{got:?}");
    assert!(!got.iter().any(|(id, _)| id == "plain"), "{got:?}");
}

#[test]
fn local_transfer_folders_are_archived_not_nested() {
    let steps = plan(&shaped(), BRAIN);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, Step::Archive { id, .. } if id == "peek"))
    );
    assert!(
        !nests(&steps)
            .iter()
            .any(|(id, under)| id == "peek" || under == "peek")
    );
    let mut busy = shaped();
    busy.iter_mut().find(|r| r.id == "peek").unwrap().running = true;
    assert!(
        !plan(&busy, BRAIN)
            .iter()
            .any(|s| matches!(s, Step::Archive { id, .. } if id == "peek"))
    );
}

#[test]
fn the_same_folder_on_several_machines_becomes_one_project() {
    let steps = plan(&shaped(), BRAIN);
    let projects: Vec<(String, Vec<String>)> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Project { project, ids, .. } => Some((project.clone(), ids.clone())),
            _ => None,
        })
        .collect();
    assert!(
        projects.contains(&("Forge".into(), vec!["forge-a".into(), "forge-b".into()])),
        "{projects:?}"
    );
    assert!(
        projects.contains(&("demo".into(), vec!["demo-b".into()])),
        "{projects:?}"
    );
    assert!(
        !projects.iter().any(|(p, _)| p == "workspace"),
        "{projects:?}"
    );
}

#[test]
fn nesting_never_makes_a_loop() {
    let rows = vec![
        row("outer", "box-1", "/srv/a", Some("inner"), None),
        row("inner", "box-1", "/srv/a/b", None, None),
    ];
    assert!(nests(&plan(&rows, BRAIN)).is_empty());
}

#[test]
fn the_count_of_top_level_workspaces_drops_by_what_folds_away() {
    let rows = shaped();
    let steps = plan(&rows, BRAIN);
    let (before, after) = tops(&rows, &steps);
    assert_eq!(before, 14);
    assert_eq!(after, 10);
}
