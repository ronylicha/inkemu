//! What the inkemu binary carries from third parties, and the notices their licences ask a
//! distribution to carry with it (Apache License 2.0, section 4; BSD-3-Clause, clause 2). The
//! texts are those of the repository, built into the binary: `inkemu licenses` prints them, and a
//! front end shows them on its "About" page.
use std::fmt::Write as _;

/// The `NOTICE` of inkemu: one paragraph per third-party part.
pub const NOTICE: &str = include_str!("../../../NOTICE");

/// A third-party part of the binary.
#[derive(Debug, PartialEq, Eq)]
pub struct ThirdParty {
    /// What it is, in a few words.
    pub name: &'static str,
    /// Where the repository keeps it, with its licence and provenance.
    pub directory: &'static str,
    pub notice: &'static str,
    pub license: &'static str,
}

/// Every third-party part the binary carries. The register descriptions are compiled into the
/// model (`svd-regs`, generated from the SVD); the bootloader is embedded byte for byte
/// (`firmware::SHIPPED`).
pub static THIRD_PARTIES: [ThirdParty; 2] = [
    ThirdParty {
        name: "ESP32-C3 register descriptions (Espressif CMSIS-SVD)",
        directory: "third_party/espressif-svd",
        notice: include_str!("../../../third_party/espressif-svd/NOTICE"),
        license: include_str!("../../../third_party/espressif-svd/LICENSE"),
    },
    ThirdParty {
        name: "ESP-IDF 2nd-stage bootloader",
        directory: "third_party/esp-idf-bootloader",
        notice: include_str!("../../../third_party/esp-idf-bootloader/NOTICE"),
        license: include_str!("../../../third_party/esp-idf-bootloader/LICENSE"),
    },
];

/// Every notice and every licence text, as one text: the `NOTICE` of inkemu, then each part with
/// its own notice and licence.
#[must_use]
pub fn notices_text() -> String {
    let mut text = String::from(NOTICE.trim_end());
    text.push('\n');
    for part in &THIRD_PARTIES {
        // Writing to a String cannot fail.
        let _ = write!(
            text,
            "\n== {} ({}) ==\n\n{}\n\n-- licence --\n\n{}\n",
            part.name,
            part.directory,
            part.notice.trim_end(),
            part.license.trim_end()
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_of_inkemu_names_every_part_and_every_part_carries_its_texts() {
        for part in &THIRD_PARTIES {
            assert!(
                NOTICE.contains(&format!("See {}/.", part.directory)),
                "the NOTICE has no paragraph for {}",
                part.directory
            );
            assert!(part.notice.contains("Espressif"), "{}", part.directory);
            assert!(
                part.license.contains("Apache License") && part.license.contains("Version 2.0"),
                "{}",
                part.directory
            );
        }
    }

    #[test]
    fn the_text_holds_every_notice_and_licence() {
        let text = notices_text();
        assert!(text.starts_with("inkemu\n"), "{text}");
        for part in &THIRD_PARTIES {
            assert!(text.contains(part.notice.trim_end()), "{}", part.directory);
            assert!(text.contains(part.license.trim_end()), "{}", part.directory);
        }
        assert!(
            text.contains("The Regents of the University of California"),
            "the BSD-3-Clause notice of the bootloader"
        );
    }
}
