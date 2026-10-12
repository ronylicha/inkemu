//! Firmware image reader and integrity checks for inkemu.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod app_desc;
pub mod application;
pub mod bootloader;
#[cfg(any(test, feature = "test-support"))]
pub mod builder;
pub mod chip;
pub mod compose;
pub mod device;
pub mod error;
pub mod flash_image;
pub mod image;
pub mod partition;

pub use app_desc::AppDesc;
pub use application::{ApplicationError, ApplicationReport, check_application};
pub use bootloader::{
    BootloaderDesc, BootloaderError, BootloaderIdentity, BootloaderReport, BootloaderWarning,
    SHIPPED, ShippedBootloader, check_bootloader,
};
pub use chip::{ChipId, FlashSize, SpiMode};
pub use compose::{Bootloader, ComposeError, ComposedFlash, compose};
pub use device::Device;
pub use error::FirmwareError;
pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};
pub use image::{FirmwareImage, Segment};
pub use partition::{Partition, PartitionError, PartitionTable};
