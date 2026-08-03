//! Shares, catalog, naming and persistence for `proton-stream`.
//!
//! Everything that is not the player or the UI. Front ends depend on this crate
//! rather than on `proton-drive-rs` directly, so there is one place that knows
//! how a share is opened and how its contents are modelled.
#![forbid(unsafe_code)]

pub mod appearance;
pub mod catalog;
pub mod config;
pub mod error;
pub mod library;
pub mod metadata;
pub mod naming;
pub mod prefs;
pub mod shares;

pub use error::{Error, Result};
pub use shares::{Share, ShareStore, SharedLibrary};

pub use proton_drive_rs;
pub use proton_sdk;
