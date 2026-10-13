//! Every test build of this crate turns on its `test-support` feature (the crate is its own
//! dev-dependency, `Cargo.toml`), and so do the documentation examples, which are compiled as
//! tests. An example that called `test_support` or `RomImage::from_flat` would therefore pass
//! `cargo test` and fail in the hands of a user, who has no such feature: the examples must take
//! what they need as parameters. This test reads the examples and refuses those two names in them.
#![allow(clippy::disallowed_methods)] // reads the sources: it is a test of the build
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// The names an example must not use: they exist only under the feature.
const TEST_ONLY: [&str; 2] = ["test_support", "from_flat"];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// The lines of the documentation examples of `text`: between the fences of `///` and `//!`
/// comments.
fn example_lines(text: &str) -> Vec<(usize, &str)> {
    let mut inside = false;
    let mut lines = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let Some(doc) = line
            .trim_start()
            .strip_prefix("///")
            .or_else(|| line.trim_start().strip_prefix("//!"))
        else {
            inside = false;
            continue;
        };
        if doc.trim_start().starts_with("```") {
            inside = !inside;
        } else if inside {
            lines.push((number + 1, doc));
        }
    }
    lines
}

#[test]
fn no_documentation_example_uses_what_only_tests_have() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(
        files.len() > 10,
        "the sources were not found under {}",
        src.display()
    );
    let mut examples = 0;
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in example_lines(&text) {
            examples += 1;
            for name in TEST_ONLY {
                assert!(
                    !line.contains(name),
                    "{}:{number}: a documentation example uses `{name}`, which only test builds have; take what it needs as a parameter",
                    file.display()
                );
            }
        }
    }
    assert!(
        examples > 0,
        "no documentation example was read: the reader is broken"
    );
}

#[test]
fn the_reader_finds_the_lines_of_an_example_and_only_them() {
    let text = "/// Text.\n///\n/// ```\n/// let rom = RomImage::from_flat(&[]);\n/// ```\n/// After.\nfn f() {}\n//! ```ignore\n//! test_support::HANG\n//! ```\n";
    assert_eq!(
        example_lines(text),
        [
            (4, " let rom = RomImage::from_flat(&[]);"),
            (9, " test_support::HANG")
        ]
    );
}
