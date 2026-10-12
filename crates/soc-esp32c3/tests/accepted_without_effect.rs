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

/// The plans a row may hand its register to: the plans still to come that own at least one row
/// of the table. A plan that is done cannot be an heir: what it did not do is handed on by name
/// before it ends, and its name is removed from this list by its last task. A plan to come that
/// owns no row (SP1b-3c and SP1b-3e today) is not in the list: it is added with its first row.
/// The list holds the table only: a comment or a document that names an heir is not read here.
const PLANS_TO_COME: [&str; 7] = ["SP1b-3d", "SP1b-3f", "SP1b-4", "SP2", "SP5a", "SP5b", "SP8"];

#[test]
fn every_row_hands_its_register_to_a_plan_that_is_still_to_come() {
    for row in accepted_without_effect() {
        assert!(
            PLANS_TO_COME.contains(&row.heir),
            "{}.{} is handed to `{}`, which is no plan to come; name the plan that will give it its effect",
            row.peripheral,
            row.register,
            row.heir
        );
    }
}
