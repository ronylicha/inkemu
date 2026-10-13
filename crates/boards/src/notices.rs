//! The third-party material inkemu embeds as data (the register descriptions of the chip and the
//! 2nd-stage bootloader it ships), and the notices their licences ask a distribution to carry
//! with it (Apache License 2.0, section 4; BSD-3-Clause, clause 2). The texts are those of the
//! repository, built into the binary: `inkemu licenses` prints them, and a front end shows them on
//! its "About" page. The notices of the Rust crates the binary links are not gathered here yet:
//! they come with the packaging of releases (SP9).
use std::fmt::Write as _;

/// The `NOTICE` of inkemu: one paragraph per third-party part.
pub const NOTICE: &str = include_str!("../../../NOTICE");

/// A piece of third-party material the binary embeds as data.
#[derive(Debug, PartialEq, Eq)]
pub struct ThirdParty {
    /// What it is, in a few words.
    pub name: &'static str,
    /// Where the repository keeps it, with its licence and provenance.
    pub directory: &'static str,
    pub notice: &'static str,
    pub license: &'static str,
}

/// Every piece of third-party material the binary embeds as data. The register descriptions are
/// compiled into the model (`svd-regs`, generated from the SVD); the bootloader is embedded byte
/// for byte (`firmware::SHIPPED`).
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

    /// The SHA-256 of the licence text kept in each third-party directory: the whole Apache
    /// License 2.0, which section 4(a) asks a distribution to give. A shortened file changes it.
    const LICENSE_SHA256: [(&str, &str); 2] = [
        (
            "third_party/espressif-svd",
            "258d0cdc26654510786f1bf1313d95e4039d220cd8ac6cdbdd973f8cb6ce831b",
        ),
        (
            "third_party/esp-idf-bootloader",
            "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
        ),
    ];

    #[test]
    #[allow(clippy::disallowed_methods)] // reads the licence files of the repository
    fn the_licence_the_binary_prints_is_the_whole_text_of_the_repository() {
        for part in &THIRD_PARTIES {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(part.directory)
                .join("LICENSE");
            let on_disk = std::fs::read(&path).unwrap();
            let embedded = firmware::bootloader::sha256_hex(part.license.as_bytes());
            assert_eq!(
                embedded,
                firmware::bootloader::sha256_hex(&on_disk),
                "the licence built into the binary is not {}; rebuild",
                path.display()
            );
            let (_, pinned) = LICENSE_SHA256
                .iter()
                .find(|(directory, _)| *directory == part.directory)
                .unwrap_or_else(|| panic!("no pinned licence hash for {}", part.directory));
            assert_eq!(
                &embedded, pinned,
                "the licence text of {} is not the Apache License 2.0 as it was added; restore \
                 the file (a deliberate change updates the pinned hash)",
                part.directory
            );
        }
        assert_eq!(LICENSE_SHA256.len(), THIRD_PARTIES.len());
    }

    #[test]
    fn the_text_holds_every_notice_and_licence() {
        let text = notices_text();
        assert!(text.starts_with("inkemu\n"), "{text}");
        for part in &THIRD_PARTIES {
            assert!(text.contains(part.notice.trim_end()), "{}", part.directory);
            assert!(text.contains(part.license.trim_end()), "{}", part.directory);
        }
    }

    /// The text with every run of white space made one space, so that a phrase is found whatever
    /// the line breaks and the indentation of the file.
    fn words(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The notice of the part kept in `directory`; a part missing from [`THIRD_PARTIES`] fails.
    fn notice_of(directory: &str) -> String {
        let part = THIRD_PARTIES
            .iter()
            .find(|part| part.directory == directory)
            .unwrap_or_else(|| panic!("no third-party part is kept in {directory}"));
        words(part.notice)
    }

    /// What clause 2 of BSD-3-Clause asks a binary copy to reproduce: the copyright notice, the
    /// list of conditions and the disclaimer, here as the headers of ESP-IDF v5.5.2 state them.
    fn assert_bsd_notice(section: &str, header: &str, copyright: &str) {
        for expected in [
            copyright,
            "Redistributions of source code must retain the above copyright notice, this list of \
             conditions and the following disclaimer.",
            "Redistributions in binary form must reproduce the above copyright notice, this list \
             of conditions and the following disclaimer in the documentation and/or other \
             materials provided with the distribution.",
            "Neither the name of the Regents nor the names of its contributors may be used to \
             endorse or promote products derived from this software without specific prior \
             written permission.",
            "IN NO EVENT SHALL REGENTS BE LIABLE TO ANY PARTY FOR DIRECT, INDIRECT, SPECIAL, \
             INCIDENTAL, OR CONSEQUENTIAL DAMAGES",
            "REGENTS SPECIFICALLY DISCLAIMS ANY WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE \
             IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE.",
        ] {
            assert!(
                section.contains(expected),
                "the notice of {header} has no `{expected}`"
            );
        }
    }

    #[test]
    fn the_register_descriptions_part_carries_its_attribution() {
        let notice = notice_of("third_party/espressif-svd");
        for expected in [
            "Copyright Espressif Systems (Shanghai) Co., Ltd.",
            "Licensed under the Apache License, Version 2.0",
        ] {
            assert!(
                notice.contains(expected),
                "the notice of the register descriptions has no `{expected}`"
            );
        }
    }

    #[test]
    fn the_bootloader_part_carries_both_bsd_notices_and_says_what_was_built_and_changed() {
        let notice = notice_of("third_party/esp-idf-bootloader");
        let csr = "components/riscv/include/riscv/csr.h:";
        let encoding = "components/riscv/include/riscv/encoding.h:";
        let (_, from_csr) = notice
            .split_once(csr)
            .unwrap_or_else(|| panic!("no section for {csr}"));
        let (csr_section, encoding_section) = from_csr
            .split_once(encoding)
            .unwrap_or_else(|| panic!("no section for {encoding}, after that of {csr}"));
        assert_bsd_notice(
            csr_section,
            csr,
            "Copyright (c) 2013, The Regents of the University of California (Regents). \
             Copyright (c) 2018-2019, The libfemto authors Copyright (c) 2020 Espressif Systems \
             (Shanghai) PTE LTD All Rights Reserved.",
        );
        assert_bsd_notice(
            encoding_section,
            encoding,
            "Copyright (c) 2010-2017, The Regents of the University of California (Regents). All \
             Rights Reserved.",
        );
        for expected in [
            "Copyright Espressif Systems (Shanghai) Co., Ltd.",
            "tag v5.5.2 (commit 30aaf64524299d3bde422ca9a2848090d1bc5d0f)",
            "built from unmodified sources",
            // The change inkemu makes to the object form, which Apache-2.0 section 4(b) asks a
            // distribution to state (`firmware::ShippedBootloader::for_flash`).
            "Apache License 2.0, section 4(b)",
            "the flash-size field of its image header",
            "the SHA-256 appended to the image is computed again",
        ] {
            assert!(
                notice.contains(expected),
                "the notice of the bootloader has no `{expected}`"
            );
        }
    }
}
