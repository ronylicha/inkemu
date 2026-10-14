//! The README's command for the lockstep is one the binary takes, with the counts the CI pins.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test: a panic is a test failure

use std::path::Path;
use std::process::Command;

fn repository_file(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The arguments after `-p inkemu-lockstep --` of the first command of `text` that has them,
/// its continuation lines joined.
fn lockstep_arguments(text: &str) -> Vec<String> {
    let joined = text.replace("\\\n", " ");
    let line = joined
        .lines()
        .find(|line| line.contains("-p inkemu-lockstep --"))
        .unwrap_or_else(|| panic!("no command of inkemu-lockstep"));
    let (_, arguments) = line.split_once("-p inkemu-lockstep --").unwrap();
    arguments.split_whitespace().map(str::to_owned).collect()
}

/// The value given to `flag` in `arguments`.
fn value_of<'a>(arguments: &'a [String], flag: &str) -> &'a str {
    let at = arguments
        .iter()
        .position(|argument| argument == flag)
        .unwrap_or_else(|| panic!("{flag} is not in {arguments:?}"));
    &arguments[at + 1]
}

#[test]
fn the_readme_runs_the_lockstep_with_flags_the_binary_takes_and_the_counts_of_the_ci() {
    let readme = repository_file("README.md");
    let section = readme
        .split_once("### For developers: the lockstep against QEMU")
        .expect("the developer section of the README")
        .1;
    let arguments = lockstep_arguments(section);
    let help = Command::new(env!("CARGO_BIN_EXE_inkemu-lockstep"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for flag in arguments
        .iter()
        .filter(|argument| argument.starts_with("--"))
    {
        assert!(
            help.contains(flag.as_str()),
            "{flag} is not an option:\n{help}"
        );
    }
    let ci = lockstep_arguments(&repository_file(".github/workflows/ci.yml"));
    for flag in ["--expect-compared", "--expect-end"] {
        assert_eq!(value_of(&arguments, flag), value_of(&ci, flag), "{flag}");
    }
}

#[test]
fn the_help_lists_every_exit_code() {
    let help = Command::new(env!("CARGO_BIN_EXE_inkemu-lockstep"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap().replace('\n', " ");
    for code in [
        "0 the window ended without a divergence",
        "1 divergence",
        "2 error",
        "3 the window ended otherwise",
        "4 inkemu stopped",
        "130 interrupted",
    ] {
        assert!(help.contains(code), "{code}:\n{help}");
    }
}
