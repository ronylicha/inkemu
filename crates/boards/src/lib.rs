//! Device profiles: what a board is made of, as data.
#![cfg_attr(test, allow(clippy::unwrap_used))]
use serde::Deserialize;
use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig};

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
}

/// Run-time choices that are not properties of the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MachineOptions {
    pub strict_mmio: bool,
    pub trace_hash: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{origin}: {detail}; correct the device profile")]
    Parse { origin: String, detail: String },
    #[error("no device named `{id}`; choose one of: {known}")]
    UnknownDevice { id: String, known: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    id: String,
    name: String,
    soc: RawSoc,
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
    DeviceProfile::from_toml(&format!("built-in profile {id}"), text)
}

impl DeviceProfile {
    /// # Errors
    /// [`ProfileError::Parse`] with the line of the first problem.
    pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError> {
        let raw: RawProfile = toml::from_str(text).map_err(|e| ProfileError::Parse {
            origin: origin.to_owned(),
            detail: e.to_string().trim_end().to_owned(),
        })?;
        Ok(DeviceProfile {
            id: raw.id,
            name: raw.name,
            soc: raw.soc.family,
            chip_revision: raw.soc.revision,
        })
    }

    #[must_use]
    pub fn soc_config(&self, options: MachineOptions) -> SocConfig {
        SocConfig {
            chip_revision: self.chip_revision,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::test_support::flat_rom;

    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n";

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
            err.contains("flash_size") && err.contains("line 7"),
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
        assert!(config.strict_mmio && config.trace_hash);
        assert_eq!(
            profile.soc_config(MachineOptions::default()),
            SocConfig {
                chip_revision: config.chip_revision,
                ..SocConfig::default()
            }
        );
    }

    #[test]
    fn instantiated_machine_sits_at_the_reset_vector() {
        let machine = builtin("x4")
            .unwrap()
            .instantiate(flat_rom(&[0x0000_006f]), MachineOptions::default());
        assert_eq!(machine.pc(), 0x4000_0000);
    }
}
