//! # funkin-assets
//!
//! First-boot setup for the port. This crate never contains game assets. It:
//!
//! 1. [`detect`]: scans common Windows install locations (Steam libraries, itch,
//!    Program Files, ...) for an official Funkin' installation.
//! 2. [`validate`]: checks that a folder really looks like an official
//!    distribution (executable, `assets/` layout, parsable song data, audio).
//! 3. [`import`]: copies the `assets/` tree into this port's private data
//!    directory (`%LOCALAPPDATA%\Rusted Mic\assets`), or records a reference to it.
//! 4. [`marker`]: writes `install.json` so later launches skip setup, and
//!    re-checks it on every boot.
//!
//! The whole crate is optional. The game binary only uses it behind the
//! `importer` cargo feature, and gameplay code only ever sees an asset root path.

pub mod detect;
pub mod fonts;
pub mod import;
pub mod marker;
pub mod paths;
pub mod validate;

pub use detect::scan_common_locations;
pub use import::{import_installation, ImportMode, ImportProgress};
pub use marker::{load_marker, resolve_asset_root, AssetStatus, InstallMarker};
pub use validate::{validate_installation, Installation, ValidationError};
