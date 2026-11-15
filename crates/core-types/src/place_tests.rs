use super::*;

#[test]
fn roots_and_homes_are_too_broad_to_hold_workspaces() {
    for p in ["/", "/root", "/mnt", "/home/me", "/Users/me/", "/home"] {
        assert!(broad(p), "{p}");
    }
    for p in [
        "/home/me/workspace",
        "/srv/Code",
        "/root/workspace",
        "/data/projects",
    ] {
        assert!(broad(p), "catch-all folders hold many projects: {p}");
    }
    for p in [
        "/mnt/share",
        "/home/me/workspace/blazar",
        "/srv/cad-repo/selfcad",
    ] {
        assert!(!broad(p), "{p}");
    }
}

#[test]
fn inside_only_counts_real_subdirectories() {
    assert!(inside("/a/b/c", "/a/b"));
    assert!(inside("/a/b/c/", "/a/b/"));
    assert!(!inside("/a/bc", "/a/b"));
    assert!(!inside("/a/b", "/a/b"));
    assert!(!inside("/a", "/a/b"));
}

#[test]
fn the_deepest_workspace_on_the_same_machine_holds_the_path() {
    let spots = [
        ("repo", "x8", "/mnt/share/me/cad-repo"),
        ("selfcad", "x8", "/mnt/share/me/cad-repo/selfcad"),
        ("other", "box", "/mnt/share/me/cad-repo/selfcad"),
        ("root", "x8", "/root"),
    ];
    assert_eq!(
        container(
            spots,
            "x8",
            "/mnt/share/me/cad-repo/selfcad/sessions/furniture"
        ),
        Some("selfcad")
    );
    assert_eq!(
        container(spots, "x8", "/mnt/share/me/cad-repo/cam"),
        Some("repo")
    );
    assert_eq!(container(spots, "x8", "/mnt/share/me/cad-repo"), None);
    assert_eq!(container(spots, "x8", "/root/planmotion"), None);
    assert_eq!(container(spots, "box", "/mnt/share/me/cad-repo/cam"), None);
}

#[test]
fn folder_is_the_last_path_part() {
    assert_eq!(folder("/home/me/workspace/Forge/"), "Forge");
    assert_eq!(folder("/"), "");
}
