//! Firmware image reader and integrity checks for inkemu.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod app_desc;
pub mod bootloader;
#[cfg(any(test, feature = "test-support"))]
pub mod builder;
pub mod chip;
pub mod error;
pub mod flash_image;
pub mod image;
pub mod partition;

pub use app_desc::AppDesc;
pub use bootloader::{BootloaderDesc, SHIPPED, ShippedBootloader};
pub use chip::{ChipId, FlashSize, SpiMode};
pub use error::FirmwareError;
pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};
pub use image::{FirmwareImage, Segment};
pub use partition::{Partition, PartitionError, PartitionTable};
