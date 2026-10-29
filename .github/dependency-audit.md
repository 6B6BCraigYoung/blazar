# Dependency audit exceptions

CI runs `cargo audit` against the full `Cargo.lock` with no ignore list. Vulnerabilities fail the build; the only waiver is the one in `.github/scripts/evaluate_audit.py`. Informational advisories (unmaintained, unsound) are reported but do not fail the build. This file records why each remaining finding cannot be fixed by an upgrade today. Review it whenever the lockfile changes.

## Waived vulnerability

| Advisory | Crate | Why it stays |
| --- | --- | --- |
| RUSTSEC-2023-0071 | rsa 0.9.10 | No patched release exists. The crate is listed in `Cargo.lock` but is not built: `cargo tree --invert rsa --target all --all-features` over normal, build and dev edges is empty, and CI waives the finding only when that check proves it. |

## Informational advisories

| Advisory | Crate | Pulled in by | Why it stays |
| --- | --- | --- | --- |
| RUSTSEC-2024-0436 | paste 1.0.15 (unmaintained) | leptos 0.8 | Compile-time macro only; leptos has not moved off it yet. |
| RUSTSEC-2024-0370 | proc-macro-error 1.0.4 (unmaintained) | gtk 0.18 via tauri on Linux | Compile-time macro only; tauri 2 pins the gtk 0.18 stack. |
| RUSTSEC-2026-0173 | proc-macro-error2 2.0.1 (unmaintained) | syn_derive via leptos_hot_reload | Compile-time macro only; waits on leptos. |
| RUSTSEC-2025-0075, RUSTSEC-2025-0080, RUSTSEC-2025-0081, RUSTSEC-2025-0098, RUSTSEC-2025-0100 | unic-char-range, unic-common, unic-char-property, unic-ucd-version, unic-ucd-ident 0.9.0 (unmaintained) | urlpattern 0.3 via tauri-utils | Used by tauri's build-time URL pattern parsing; waits on tauri. |
| RUSTSEC-2024-0429 | glib 0.18.5 (unsound) | gtk 0.18 via tauri on Linux | Fixed in glib 0.20, which tauri 2 does not use yet. Blazar does not call `glib::VariantStrIter`. |
