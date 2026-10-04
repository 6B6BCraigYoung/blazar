#[test]
fn terminal_login_hints_never_use_an_html_sink() {
    let dialog = include_str!("../src/components/term_dialog.rs");
    assert!(
        !dialog.contains("inner_html"),
        "terminal login hints contain machine names and must render as text"
    );
}
