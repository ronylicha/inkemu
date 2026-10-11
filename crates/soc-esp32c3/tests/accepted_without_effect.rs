//! Pins `docs/registers-accepted-without-effect.md` to the lists the peripheral models are built from.
//!
//! When the lists change, `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`
//! rewrites the table of that file, and of nothing else; review its diff, then commit it.
use soc_esp32c3::periph::{accepted_without_effect, render_accepted_table};
use std::path::Path;

/// The document, relative to the directory of this crate.
const DOCUMENT: &str = "../../docs/registers-accepted-without-effect.md";
/// The line of the document after which everything is the generated table.
const MARKER: &str = "<!-- generated table -->";
const BLESS: &str = "INKEMU_BLESS";

#[test]
fn the_documented_table_is_the_one_the_code_builds() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(DOCUMENT);
    let document = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let (prose, rest) = document.split_once(MARKER).unwrap_or_else(|| {
        panic!(
            "{} has no `{MARKER}` line; restore it above the table",
            path.display()
        )
    });
    let table = render_accepted_table(&accepted_without_effect());
    if rest.strip_prefix('\n').unwrap_or(rest) == table {
        return;
    }
    assert!(
        std::env::var_os(BLESS).is_some_and(|value| value == "1"),
        "docs/registers-accepted-without-effect.md is stale; run `{BLESS}=1 cargo test -p soc-esp32c3 --test accepted_without_effect` to rewrite its table, then review the diff of that file"
    );
    std::fs::write(&path, format!("{prose}{MARKER}\n{table}"))
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}
