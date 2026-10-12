//! Device profiles: what a board is made of, as data.
#![cfg_attr(test, allow(clippy::unwrap_used))]
use serde::Deserialize;
use soc_esp32c3::flash::{
    FlashConfig, FlashConfigError, FlashImageError, JedecId, MAX_FLASH_BYTES, MIB, MIN_FLASH_BYTES,
};
use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig};
use std::fmt;

/// How to write the `jedec_id` key, for the messages that refuse one.
const JEDEC_ID_FORM: &str = "write jedec_id as 0xMMTTCC (manufacturer, type, capacity)";

const BUILTIN: [(&str, &str); 2] = [
    ("x3", include_str!("../profiles/x3.toml")),
    ("x4", include_str!("../profiles/x4.toml")),
];
/// The ids of [`BUILTIN`], in the same order: derived, so there is one list to keep.
const BUILTIN_IDS: [&str; BUILTIN.len()] = {
    let mut ids = [""; BUILTIN.len()];
    let mut i = 0;
    while i < BUILTIN.len() {
        ids[i] = BUILTIN[i].0;
        i += 1;
    }
    ids
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SocFamily {
    Esp32C3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub id: String,
    pub name: String,
    pub soc: SocFamily,
    pub chip_revision: ChipRevision,
    pub flash: FlashConfig,
}

/// Run-time choices that are not properties of the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MachineOptions {
    pub strict_mmio: bool,
    pub trace_hash: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// `detail` ends with a remedy: its own, or the general one ([`parse_detail`]).
    #[error("{origin}: {detail}")]
    Parse { origin: String, detail: String },
    #[error(
        "{origin}: the profile has no [flash] table; add one with its two keys: size_mib ({sizes}) and jedec_id (0xMMTTCC, the three bytes the chip answers to a read-id command)",
        sizes = flash_sizes_mib()
    )]
    MissingFlash { origin: String },
    #[error("no device named `{id}`; choose one of: {known}")]
    UnknownDevice { id: String, known: String },
    #[error(
        "{origin}: the profile declares the id `{found}` but is registered as `{expected}`; make the id of the profile match the name it is loaded under"
    )]
    IdMismatch {
        origin: String,
        expected: String,
        found: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(deserialize_with = "profile_id")]
    id: String,
    name: String,
    soc: RawSoc,
    /// Optional here so that its absence gets a message that names the table and its keys.
    flash: Option<RawFlash>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedFlash {
    #[serde(deserialize_with = "flash_size")]
    size_mib: u32,
    #[serde(deserialize_with = "jedec_id")]
    jedec_id: JedecId,
}

/// The `[flash]` table once its two keys agree with each other.
#[derive(Deserialize)]
#[serde(try_from = "UncheckedFlash")]
struct RawFlash(FlashConfig);

impl TryFrom<UncheckedFlash> for RawFlash {
    type Error = String;

    /// [`FlashConfig::new`] owns the rules; this turns its refusals into messages that name the
    /// keys of the profile. A size is refused earlier, by `flash_size`, with its own line.
    fn try_from(raw: UncheckedFlash) -> Result<RawFlash, String> {
        let size = raw
            .size_mib
            .checked_mul(MIB)
            .ok_or_else(|| unmodelable_size(i64::from(raw.size_mib)))?;
        FlashConfig::new(size, raw.jedec_id)
            .map(RawFlash)
            .map_err(|refusal| match refusal {
                FlashConfigError::BadSize { .. } => unmodelable_size(i64::from(raw.size_mib)),
                // The third JEDEC byte is log2 of the capacity: a profile that contradicts itself
                // would make the bootloader and the flash driver disagree about the size.
                FlashConfigError::CapacityMismatch { id, capacity, .. } => format!(
                    "the flash is {} MiB but the JEDEC id {id:#08x} ends with capacity byte {capacity:#04x}, which stands for another size; make size_mib and the last byte of jedec_id agree (0x14 = 1 MiB … 0x18 = 16 MiB)",
                    raw.size_mib
                ),
            })
    }
}

/// The flash sizes the machine models, in MiB, as a message lists them: "1, 2, 4, 8 or 16".
fn flash_sizes_mib() -> String {
    let sizes: Vec<String> =
        std::iter::successors(Some(MIN_FLASH_BYTES), |size| size.checked_mul(2))
            .take_while(|&size| size <= MAX_FLASH_BYTES)
            .filter(|&size| FlashConfig::size_is_valid(size))
            .map(|size| (size / MIB).to_string())
            .collect();
    match sizes.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        Some((only, _)) => only.clone(),
        None => String::new(),
    }
}

fn unmodelable_size(mib: i64) -> String {
    format!(
        "a flash of {mib} MiB cannot be modelled; write size_mib as {}",
        flash_sizes_mib()
    )
}

/// A TOML integer. Any other type is refused with `how`, the way to write the key, as the remedy.
struct WholeNumber {
    how: String,
}

impl serde::de::Visitor<'_> for WholeNumber {
    type Value = i64;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a whole number; {}", self.how)
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<i64, E> {
        Ok(value)
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<i64, E> {
        // Past `i64::MAX` the exact value no longer matters: no key accepts it.
        Ok(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

/// A device id names files and appears in messages: lower-case letters, digits and `-` only.
fn profile_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(id)
    } else {
        Err(serde::de::Error::custom(format!(
            "the device id `{}` is empty or has characters other than a-z, 0-9 and `-`; write an id such as x4",
            id.escape_debug()
        )))
    }
}

/// `size_mib`, validated where the TOML error can still carry its line. The result is in MiB.
fn flash_size<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let written = deserializer.deserialize_any(WholeNumber {
        how: format!("write size_mib as {}", flash_sizes_mib()),
    })?;
    u32::try_from(written)
        .ok()
        .filter(|mib| mib.checked_mul(MIB).is_some_and(FlashConfig::size_is_valid))
        .ok_or_else(|| serde::de::Error::custom(unmodelable_size(written)))
}

fn jedec_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<JedecId, D::Error> {
    let written = deserializer.deserialize_any(WholeNumber {
        how: JEDEC_ID_FORM.to_owned(),
    })?;
    u32::try_from(written)
        .ok()
        .and_then(JedecId::from_u24)
        .ok_or_else(|| {
            // A negative id is shown in decimal, as it was most likely written.
            let shown = if written < 0 {
                written.to_string()
            } else {
                format!("{written:#x}")
            };
            serde::de::Error::custom(format!(
                "the JEDEC id {shown} does not fit three bytes; {JEDEC_ID_FORM}"
            ))
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSoc {
    family: SocFamily,
    #[serde(deserialize_with = "chip_revision")]
    revision: ChipRevision,
}

/// Parsing inside the deserializer lets the TOML error carry the line of the offending value.
fn chip_revision<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ChipRevision, D::Error> {
    String::deserialize(deserializer)?
        .parse()
        .map_err(serde::de::Error::custom)
}

/// What a refusal of the TOML says, with one remedy. The deserializers of this crate end their
/// message with a remedy of their own (`…; write size_mib as …`); the parser's messages (an unknown
/// key, a missing one, broken TOML) have none and get the general one. The message is the last
/// line: the lines above it show where in the file the problem is.
fn parse_detail(error: &toml::de::Error) -> String {
    let detail = error.to_string().trim_end().to_owned();
    let has_remedy = detail
        .lines()
        .next_back()
        .is_some_and(|message| message.contains("; "));
    if has_remedy {
        detail
    } else {
        format!("{detail}; correct the device profile")
    }
}

#[must_use]
pub fn builtin_ids() -> &'static [&'static str] {
    &BUILTIN_IDS
}

/// # Errors
/// [`ProfileError::UnknownDevice`] when no built-in profile has this id.
pub fn builtin(id: &str) -> Result<DeviceProfile, ProfileError> {
    let (_, text) = BUILTIN
        .iter()
        .find(|(known, _)| *known == id)
        .ok_or_else(|| ProfileError::UnknownDevice {
            id: id.to_owned(),
            known: BUILTIN_IDS.join(", "),
        })?;
    DeviceProfile::from_named_toml(id, &format!("built-in profile {id}"), text)
}

impl DeviceProfile {
    /// # Errors
    /// [`ProfileError::Parse`] with the line of the first problem, or
    /// [`ProfileError::MissingFlash`] when the profile has no `[flash]` table.
    pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError> {
        let raw: RawProfile = toml::from_str(text).map_err(|e| ProfileError::Parse {
            origin: origin.to_owned(),
            detail: parse_detail(&e),
        })?;
        let RawFlash(flash) = raw.flash.ok_or_else(|| ProfileError::MissingFlash {
            origin: origin.to_owned(),
        })?;
        Ok(DeviceProfile {
            id: raw.id,
            name: raw.name,
            soc: raw.soc.family,
            chip_revision: raw.soc.revision,
            flash,
        })
    }

    /// Like [`DeviceProfile::from_toml`], and checks that the profile carries the id it is loaded under.
    ///
    /// # Errors
    /// [`ProfileError::Parse`] or [`ProfileError::MissingFlash`] as [`DeviceProfile::from_toml`]
    /// says, or [`ProfileError::IdMismatch`] when the ids differ.
    pub fn from_named_toml(
        expected_id: &str,
        origin: &str,
        text: &str,
    ) -> Result<DeviceProfile, ProfileError> {
        let profile = DeviceProfile::from_toml(origin, text)?;
        if profile.id == expected_id {
            Ok(profile)
        } else {
            Err(ProfileError::IdMismatch {
                origin: origin.to_owned(),
                expected: expected_id.to_owned(),
                found: profile.id,
            })
        }
    }

    #[must_use]
    pub fn soc_config(&self, options: MachineOptions) -> SocConfig {
        SocConfig {
            chip_revision: self.chip_revision,
            flash: self.flash,
            strict_mmio: options.strict_mmio,
            trace_hash: options.trace_hash,
            ..SocConfig::default()
        }
    }

    /// Builds the device in its power-on reset state.
    #[must_use]
    pub fn instantiate(&self, rom: RomImage, options: MachineOptions) -> Esp32C3 {
        match self.soc {
            SocFamily::Esp32C3 => Esp32C3::new(rom, &self.soc_config(options)),
        }
    }

    /// Builds the device in its power-on reset state with `image` in its flash from offset 0.
    ///
    /// # Errors
    /// [`FlashImageError`] when the image is longer than the flash of the device.
    pub fn instantiate_with_flash(
        &self,
        rom: RomImage,
        image: Vec<u8>,
        options: MachineOptions,
    ) -> Result<Esp32C3, FlashImageError> {
        match self.soc {
            SocFamily::Esp32C3 => Esp32C3::with_flash(rom, image, &self.soc_config(options)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::test_support::{HANG, flat_rom};

    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n\n[flash]\nsize_mib = 4\njedec_id = 0xEF4016\n";

    #[test]
    fn builtin_profiles_load_and_carry_their_own_id() {
        assert_eq!(builtin_ids(), ["x3", "x4"]);
        assert_eq!(
            builtin_ids(),
            BUILTIN.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "one list of devices"
        );
        for id in builtin_ids() {
            let profile = builtin(id).unwrap();
            assert_eq!(profile.id, *id);
            assert_eq!(profile.soc, SocFamily::Esp32C3);
            assert_eq!(profile.chip_revision, ChipRevision::V0_3);
            assert!(profile.name.starts_with("Xteink"));
            assert_eq!(profile.flash.size(), 16 << 20, "spec §2: 16 MB of flash");
            assert_eq!(
                profile.flash.jedec_id(),
                soc_esp32c3::JedecId::QEMU_ORACLE,
                "decision D2"
            );
        }
    }

    #[test]
    fn unknown_device_lists_the_known_ones() {
        let err = builtin("x5").unwrap_err().to_string();
        assert!(
            err.contains("x5") && err.contains("x3") && err.contains("x4"),
            "{err}"
        );
    }

    #[test]
    fn valid_profile_parses() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        assert_eq!(
            (profile.id.as_str(), profile.name.as_str()),
            ("demo", "Demo board")
        );
        assert_eq!(profile.chip_revision.to_string(), "v1.1");
    }

    fn error_for(text: &str) -> String {
        DeviceProfile::from_toml("demo.toml", text)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn a_refused_profile_states_one_remedy() {
        // A refusal of this crate carries its own remedy, and no other is added after it.
        let size = error_for(&VALID.replace("size_mib = 4", "size_mib = 3"));
        assert!(
            size.ends_with("; write size_mib as 1, 2, 4, 8 or 16"),
            "{size}"
        );
        assert!(!size.contains("correct the device profile"), "{size}");
        // A refusal of the parser has none, and gets the general one, once.
        let key = error_for(&format!("{VALID}x = 1\n"));
        assert!(
            key.contains("unknown field `x`") && key.ends_with("; correct the device profile"),
            "{key}"
        );
        assert_eq!(
            key.matches("; correct the device profile").count(),
            1,
            "{key}"
        );
    }

    #[test]
    fn malformed_revision_points_at_its_line_and_says_how_to_write_it() {
        let err = error_for(&VALID.replace("v1.1", "1.1"));
        assert!(
            err.contains("demo.toml") && err.contains("line 6") && err.contains("v<major>.<minor>"),
            "{err}"
        );
    }

    #[test]
    fn unknown_soc_family_points_at_its_line_and_lists_the_supported_ones() {
        let err = error_for(&VALID.replace("esp32c3", "esp8266"));
        assert!(
            err.contains("line 5") && err.contains("esp8266") && err.contains("esp32c3"),
            "{err}"
        );
    }

    #[test]
    fn unknown_key_is_rejected_rather_than_ignored() {
        let err = error_for(&format!("{VALID}flash_size = \"16MB\"\n"));
        assert!(
            err.contains("flash_size") && err.contains("line 11"),
            "{err}"
        );
    }

    #[test]
    fn missing_key_and_broken_toml_are_reported_with_the_file_name() {
        let missing =
            error_for("id = \"demo\"\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v0.3\"\n");
        assert!(
            missing.contains("demo.toml") && missing.contains("`name`"),
            "{missing}"
        );
        let broken = error_for("id = \n");
        assert!(
            broken.contains("demo.toml") && broken.contains("line 1"),
            "{broken}"
        );
    }

    #[test]
    fn options_and_profile_combine_into_the_soc_configuration() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        let config = profile.soc_config(MachineOptions {
            strict_mmio: true,
            trace_hash: true,
        });
        assert_eq!(config.chip_revision.to_string(), "v1.1");
        assert_eq!(config.flash, profile.flash);
        assert!(config.strict_mmio && config.trace_hash);
        assert_eq!(
            profile.soc_config(MachineOptions::default()),
            SocConfig {
                chip_revision: config.chip_revision,
                flash: profile.flash,
                ..SocConfig::default()
            }
        );
    }

    /// `VALID` with one line replaced.
    fn with_line(number: usize, replacement: &str) -> String {
        VALID
            .lines()
            .enumerate()
            .map(|(i, line)| if i + 1 == number { replacement } else { line })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn valid_profile_carries_its_flash() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        assert_eq!(profile.flash.size(), 4 << 20);
        assert_eq!(profile.flash.jedec_id().bytes(), [0xEF, 0x40, 0x16]);
    }

    #[test]
    fn flash_size_must_be_a_power_of_two_up_to_16_mib() {
        for bad in ["0", "3", "32", "-4", "4.0", "\"16MB\"", "4294967296"] {
            let err = error_for(&with_line(9, &format!("size_mib = {bad}")));
            assert!(
                err.contains("line 9") && err.contains("; write size_mib as 1, 2, 4, 8 or 16"),
                "{bad}: {err}"
            );
        }
        // The cause names what was written.
        for (bad, cause) in [
            ("3", "a flash of 3 MiB cannot be modelled"),
            ("-4", "a flash of -4 MiB cannot be modelled"),
            ("4.0", "floating point `4.0`"),
            ("\"16MB\"", "string \"16MB\""),
        ] {
            let err = error_for(&with_line(9, &format!("size_mib = {bad}")));
            assert!(err.contains(cause), "{bad}: {err}");
        }
    }

    #[test]
    fn the_list_of_sizes_in_the_remedy_is_every_size_the_machine_models() {
        let listed = flash_sizes_mib();
        assert_eq!(listed, "1, 2, 4, 8 or 16");
        for mib in 0..=64u32 {
            let named = listed
                .replace(" or ", ", ")
                .split(", ")
                .any(|size| size == mib.to_string());
            assert_eq!(named, FlashConfig::size_is_valid(mib * MIB), "{mib} MiB");
        }
    }

    #[test]
    fn jedec_id_must_be_three_bytes() {
        for bad in ["0x1EF4016", "-1", "9.5", "\"0x9D6018\""] {
            let err = error_for(&with_line(10, &format!("jedec_id = {bad}")));
            assert!(
                err.contains("line 10")
                    && err.contains("; write jedec_id as 0xMMTTCC (manufacturer, type, capacity)"),
                "{bad}: {err}"
            );
        }
        for (bad, cause) in [
            (
                "0x1EF4016",
                "the JEDEC id 0x1ef4016 does not fit three bytes",
            ),
            ("-1", "the JEDEC id -1 does not fit three bytes"),
            ("\"0x9D6018\"", "string \"0x9D6018\""),
        ] {
            let err = error_for(&with_line(10, &format!("jedec_id = {bad}")));
            assert!(err.contains(cause), "{bad}: {err}");
        }
    }

    #[test]
    fn jedec_capacity_must_match_the_flash_size() {
        // 0x16 is 4 MiB; the profile says 8. The two keys disagree with each other, so the error
        // points at the table that holds both: `[flash]` is line 8 of `VALID`.
        let err = error_for(&with_line(9, "size_mib = 8"));
        assert!(
            err.contains("line 8")
                && err.contains(
                    "the flash is 8 MiB but the JEDEC id 0xef4016 ends with capacity byte 0x16"
                )
                && err.contains("; make size_mib and the last byte of jedec_id agree"),
            "{err}"
        );
    }

    #[test]
    fn the_flash_table_is_required_and_the_error_names_it_with_its_keys() {
        let without = VALID.split("\n[flash]").next().unwrap();
        let err = DeviceProfile::from_toml("demo.toml", without).unwrap_err();
        assert_eq!(
            err,
            ProfileError::MissingFlash {
                origin: "demo.toml".into()
            }
        );
        let text = err.to_string();
        assert!(
            text.starts_with("demo.toml: the profile has no [flash] table; add one")
                && text.contains("size_mib")
                && text.contains("1, 2, 4, 8 or 16")
                && text.contains("jedec_id")
                && text.contains("0xMMTTCC"),
            "{text}"
        );
    }

    #[test]
    fn empty_or_malformed_id_is_refused_with_its_line() {
        // TOML source lines: the fourth id holds an escape character (written with a TOML escape).
        for line in [
            r#"id = """#,
            r#"id = "X4""#,
            r#"id = "x 4""#,
            r#"id = "x4\u001b[31m""#,
            r#"id = "../x4""#,
        ] {
            let err = error_for(&with_line(1, line));
            assert!(
                err.contains("line 1") && err.contains("a-z, 0-9"),
                "{line}: {err}"
            );
            assert!(!err.contains('\u{1b}'), "control characters are not echoed");
        }
    }

    #[test]
    fn a_profile_registered_under_another_id_is_refused() {
        let err = DeviceProfile::from_named_toml("x9", "built-in profile x9", VALID).unwrap_err();
        assert_eq!(
            err,
            ProfileError::IdMismatch {
                origin: "built-in profile x9".into(),
                expected: "x9".into(),
                found: "demo".into()
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("`demo`") && text.contains("`x9`") && text.contains("; make"),
            "{text}"
        );
        assert!(DeviceProfile::from_named_toml("demo", "demo.toml", VALID).is_ok());
    }

    #[test]
    fn a_machine_can_be_instantiated_around_a_flash_image() {
        let profile = builtin("x4").unwrap();
        let ok = profile.instantiate_with_flash(
            flat_rom(&[HANG]),
            vec![0xE9; 32],
            MachineOptions::default(),
        );
        assert!(ok.is_ok());
        let too_long = vec![0u8; (16 << 20) + 1];
        let err = profile
            .instantiate_with_flash(flat_rom(&[HANG]), too_long, MachineOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("16777217"), "{err}");
    }

    /// The loader is the only way from text to a profile: whatever it accepts has a flash that the
    /// machine can model and whose JEDEC capacity byte agrees with its size, so no profile loaded
    /// from text carries an invalid or inconsistent flash into `instantiate` or
    /// `instantiate_with_flash`.
    #[test]
    fn no_text_yields_a_profile_whose_flash_is_invalid_or_inconsistent() {
        let sizes = [
            "0",
            "1",
            "2",
            "3",
            "4",
            "5",
            "8",
            "16",
            "17",
            "32",
            "4294967296",
        ];
        let ids = [
            "0x000000", "0xEF4014", "0xEF4015", "0xEF4016", "0x9D6018", "0xEF40FF",
        ];
        for size in sizes {
            for id in ids {
                let text = with_line(9, &format!("size_mib = {size}"))
                    .replace("jedec_id = 0xEF4016", &format!("jedec_id = {id}"));
                // The oracle is computed from the text alone, independently of the loader.
                let jedec = u32::from_str_radix(&id[2..], 16).unwrap();
                let consistent = size.parse::<u32>().is_ok_and(|mib| {
                    mib.is_power_of_two()
                        && mib <= 16
                        && 1u32.checked_shl(jedec & 0xFF) == Some(mib << 20)
                });
                let loaded = DeviceProfile::from_toml("demo.toml", &text);
                assert_eq!(
                    loaded.is_ok(),
                    consistent,
                    "{size} MiB {id}: the loader and the oracle disagree ({loaded:?})"
                );
                if let Ok(profile) = loaded {
                    let mib: u32 = size.parse().unwrap();
                    assert_eq!(profile.flash.size(), mib << 20, "{size} MiB {id}");
                    assert_eq!(profile.flash.jedec_id().to_u24(), jedec, "{size} MiB {id}");
                    // The machine accepts it, with a smaller image and through both entry points.
                    let options = MachineOptions::default();
                    let machine = profile.instantiate(flat_rom(&[HANG]), options);
                    assert_eq!(machine.pc(), 0x4000_0000);
                    assert!(
                        profile
                            .instantiate_with_flash(flat_rom(&[HANG]), vec![0xE9; 8], options)
                            .is_ok()
                    );
                }
            }
        }
        // Every accepted pair is exercised at least once: 1, 2, 4, 8, 16 MiB.
        for (mib, capacity) in [(1, 0x14), (2, 0x15), (4, 0x16), (8, 0x17), (16, 0x18)] {
            let text = with_line(9, &format!("size_mib = {mib}")).replace(
                "jedec_id = 0xEF4016",
                &format!("jedec_id = 0xEF40{capacity:02X}"),
            );
            assert!(
                DeviceProfile::from_toml("demo.toml", &text).is_ok(),
                "{mib} MiB"
            );
        }
    }

    #[test]
    fn instantiated_machine_sits_at_the_reset_vector() {
        let machine = builtin("x4")
            .unwrap()
            .instantiate(flat_rom(&[HANG]), MachineOptions::default());
        assert_eq!(machine.pc(), 0x4000_0000);
    }
}
