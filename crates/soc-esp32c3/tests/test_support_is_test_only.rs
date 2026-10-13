//! Every test build of this crate turns on its `test-support` feature (the crate is its own
//! dev-dependency, `Cargo.toml`), and so do the documentation examples, which are compiled as
//! tests; the dev-dependencies turn on the same feature of `cpu-rv32` and `firmware`. An example
//! that called `test_support`, `RomImage::from_flat`, `cpu_rv32::asm` or `firmware::builder`
//! would therefore pass `cargo test` and fail in the hands of a user, who has no such feature:
//! the examples must take what they need as parameters. This test reads the examples and refuses
//! those names in them.
#![allow(clippy::disallowed_methods)] // reads the sources: it is a test of the build
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// The names an example must not use, each an identifier: they exist only under the features.
/// `test_support` and `RomImage::from_flat` are this crate's (`src/lib.rs`, `src/rom.rs`); `asm`
/// is the module of `cpu-rv32` and `builder` that of `firmware` (their `src/lib.rs`).
const TEST_ONLY: [&str; 4] = ["test_support", "from_flat", "asm", "builder"];

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

/// The string of a one-line documentation attribute, from what follows `#[doc` or `#![doc`:
/// ` = "text"]` gives `text`, escapes left as they are.
fn attribute_string(rest: &str) -> Option<&str> {
    let value = rest.trim_start().strip_prefix('=')?.trim_start();
    let value = value.strip_prefix('"')?;
    Some(value.rsplit_once('"').map_or(value, |(text, _)| text))
}

/// The documentation each line of `text` carries, if any, with its line number: what follows
/// `///` or `//!`, the string of a one-line `#[doc = "…"]` or `#![doc = "…"]`, or a line of a
/// `/** … */` or `/*! … */` block, its leading `*` dropped.
fn doc_lines(text: &str) -> Vec<(usize, Option<&str>)> {
    let mut in_block = false;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let doc = if in_block {
            let body = match trimmed.split_once("*/") {
                Some((body, _)) => {
                    in_block = false;
                    body
                }
                None => trimmed,
            };
            Some(body.strip_prefix('*').unwrap_or(body))
        } else if trimmed.starts_with("/**/") || trimmed.starts_with("/***") {
            // An empty block comment, or a rule of stars: no documentation.
            None
        } else if let Some(rest) = trimmed
            .strip_prefix("/**")
            .or_else(|| trimmed.strip_prefix("/*!"))
        {
            if let Some((body, _)) = rest.split_once("*/") {
                Some(body)
            } else {
                in_block = true;
                Some(rest)
            }
        } else if let Some(rest) = trimmed
            .strip_prefix("///")
            .or_else(|| trimmed.strip_prefix("//!"))
        {
            Some(rest)
        } else if let Some(rest) = trimmed
            .strip_prefix("#[doc")
            .or_else(|| trimmed.strip_prefix("#![doc"))
        {
            attribute_string(rest)
        } else {
            None
        };
        lines.push((index + 1, doc));
    }
    lines
}

/// The lines of the documentation examples of `text`: between two fences of its documentation.
fn example_lines(text: &str) -> Vec<(usize, &str)> {
    let mut inside = false;
    let mut lines = Vec::new();
    for (number, doc) in doc_lines(text) {
        let Some(doc) = doc else {
            inside = false;
            continue;
        };
        if doc.trim_start().starts_with("```") {
            inside = !inside;
        } else if inside {
            lines.push((number, doc));
        }
    }
    lines
}

/// Whether `name` appears in `line` as a whole identifier: `asm::lui` and `{asm, x}` name `asm`,
/// `plasma` does not.
fn names(line: &str, name: &str) -> bool {
    let part_of_identifier = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(name).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + name.len()..].chars().next();
        !before.is_some_and(part_of_identifier) && !after.is_some_and(part_of_identifier)
    })
}

/// Each use in an example of `text` of a name of [`TEST_ONLY`], with its line number.
fn test_only_uses(text: &str) -> Vec<(usize, &'static str)> {
    example_lines(text)
        .into_iter()
        .flat_map(|(number, line)| {
            TEST_ONLY
                .into_iter()
                .filter(move |name| names(line, name))
                .map(move |name| (number, name))
        })
        .collect()
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
        assert!(
            !text
                .lines()
                .any(|line| line.contains("doc") && line.contains("include_str!")),
            "{}: documentation is included from another file, which this test does not read; write it in the source, or make the test read that file",
            file.display()
        );
        examples += example_lines(&text).len();
        let uses = test_only_uses(&text);
        assert!(
            uses.is_empty(),
            "{}: documentation examples use what only test builds have, at (line, name) {uses:?}; take what they need as parameters",
            file.display()
        );
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

#[test]
fn the_reader_finds_the_examples_of_block_comments_and_attributes() {
    let text = "/**\n * ```\n * let a = 1;\n * ```\n */\nfn f() {}\n/*! ```\nlet b = 2;\n``` */\n#[doc = \"```\"]\n#[doc = \"let c = 3;\"]\n#![doc = \"```\"]\n/* ```\nlet d = 4;\n``` */\n";
    assert_eq!(
        example_lines(text),
        [(3, " let a = 1;"), (8, "let b = 2;"), (11, "let c = 3;")]
    );
}

#[test]
fn every_name_only_tests_have_is_found_in_an_example() {
    let text = "/// ```\n/// let rom = RomImage::from_flat(vec![0; 4]);\n/// let hang = soc_esp32c3::test_support::HANG;\n/// use cpu_rv32::{asm, decode};\n/// let image = firmware::builder::build_image(&spec);\n/// let plasma = test_supports + from_flat_rom + builders;\n/// ```\n";
    assert_eq!(
        test_only_uses(text),
        [
            (2, "from_flat"),
            (3, "test_support"),
            (4, "asm"),
            (5, "builder")
        ]
    );
}
