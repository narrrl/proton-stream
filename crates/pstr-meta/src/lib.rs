//! Matching a library's titles against a metadata provider.
//!
//! What this crate is for is the gap between what a filename says and what a
//! poster wall wants: a share filled by the Proton Drive Linux client carries no
//! thumbnails at all — Proton renders those at upload time and that client
//! attaches none — so without enrichment every tile in the grid is a pair of
//! initials on a grey rectangle. See `docs/DEVELOPMENT.md`.
//!
//! ```text
//!   Title ──▶ Query ──▶ Provider::search ──▶ [Candidate] ──▶ matching::best
//!                        (anilist | tmdb)                        │
//!                                                                ▼
//!                                                        TitleMetadata
//! ```
//!
//! Three things are worth knowing before touching any of it:
//!
//! * **It is off by default and says why.** Enrichment sends the titles in
//!   someone's library to a third party. The switch is theirs, the reasoning is
//!   in `pstr_core::metadata`, and nothing here runs until it is on.
//! * **A wrong match is worse than no match.** The wrong poster does not look
//!   like a bug, it looks like the library is wrong — so
//!   [`matching::MATCH_FLOOR`] is high and a weak field yields nothing.
//! * **Misses are cached and failures are not.** See [`service`].
//!
//! The model — [`pstr_core::metadata::TitleMetadata`] and friends — deliberately
//! lives in `pstr-core`, so the catalog can store it and the UI can draw it
//! without either depending on this crate.

pub mod anilist;
pub mod error;
pub mod matching;
pub mod provider;
pub mod service;
pub mod settings;
pub mod tmdb;

pub use error::{Error, Result};
pub use matching::{Candidate, Query};
pub use provider::Provider;
pub use service::MetadataService;
