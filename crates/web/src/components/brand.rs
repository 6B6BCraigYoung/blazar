//! The Blazar mark, drawn the same way everywhere (see `docs/design/brand.md`): a horizon arc
//! open at the bottom and two jets leaving it. The app icon, the sidebar logo and the about page
//! use the tile; empty states use the line form around the page's own icon.

/// The arc and the jets on the 1024 grid of the app icon (`apps/desktop/icons/src`).
const MARK: &str = r##"<g fill="none" stroke="#fff" stroke-linecap="round" transform="translate(0 24)"><path d="M365 616 A208 208 0 1 1 659 616" stroke-width="46"/><path d="M471 478 L449 690 M553 478 L575 690" stroke-width="54"/></g>"##;

/// The mark on its indigo tile, cropped to the tile so it can be sized by CSS.
#[must_use]
pub fn tile() -> String {
    format!(
        r##"<svg viewBox="100 100 824 824" aria-hidden="true"><rect x="100" y="100" width="824" height="824" rx="185" fill="#5e5ce6"/>{MARK}</svg>"##
    )
}

/// The line form for empty states: the arc in a quiet grey around a 24px icon, the jets in the
/// accent under it. Colours come from CSS (`.blank-arc`, `.blank-jet`).
pub const HORIZON: &str = r#"<svg viewBox="0 0 64 64" aria-hidden="true"><path class="blank-arc" d="M15.03 46.97 A24 24 0 1 1 48.97 46.97"/><path class="blank-jet" d="M28 45 L26.8 57 M36 45 L37.2 57"/></svg>"#;
