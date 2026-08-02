//! Reading and writing the enrichment settings, and the API key.
//!
//! The settings file sits beside `shares.json` and is written the same way —
//! atomically, and never overwritten when it will not parse. The API key does
//! *not* go in it: it is a credential, it belongs in the OS credential store,
//! and the same rule that keeps share fragments out of the config keeps this out
//! too.

use pstr_core::config::{AppDirs, read_json, write_json};
use pstr_core::metadata::{MetadataConfig, ProviderId};

use crate::error::{Error, Result};

/// The credential-store service name, shared with [`pstr_core::shares`]. Keys
/// are namespaced by provider, so two providers' keys can coexist and switching
/// back does not mean typing one in again.
const KEYRING_SERVICE: &str = "proton-stream";

fn settings_file(dirs: &AppDirs) -> std::path::PathBuf {
    dirs.config.join("metadata.json")
}

fn keyring_entry(provider: ProviderId) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("metadata-{}", provider.as_str()))
        .map_err(|error| Error::Config(format!("open the credential store: {error}")))
}

/// The stored settings, or the defaults on a first run.
pub fn load(dirs: &AppDirs) -> Result<MetadataConfig> {
    Ok(read_json(&settings_file(dirs))?.unwrap_or_default())
}

/// Write the settings.
pub fn save(dirs: &AppDirs, config: &MetadataConfig) -> Result<()> {
    write_json(&settings_file(dirs), config)?;
    Ok(())
}

/// The stored API key for a provider, if there is one.
///
/// A credential store that is locked or absent is reported as *no key* rather
/// than as an error: the caller's next move is the same either way — tell the
/// viewer the provider needs a key — and a keyring failure phrased as one is a
/// worse message than that.
pub fn api_key(provider: ProviderId) -> Option<String> {
    if !provider.needs_api_key() {
        return None;
    }
    match keyring_entry(provider).and_then(|entry| {
        entry
            .get_password()
            .map_err(|error| Error::Config(error.to_string()))
    }) {
        Ok(key) => Some(key),
        Err(error) => {
            tracing::debug!("no stored API key for {}: {error}", provider.label());
            None
        }
    }
}

/// Store an API key, or forget it when `key` is empty.
pub fn set_api_key(provider: ProviderId, key: &str) -> Result<()> {
    let entry = keyring_entry(provider)?;
    let result = if key.trim().is_empty() {
        match entry.delete_credential() {
            // Clearing a key that was never set is what the viewer asked for.
            Err(keyring::Error::NoEntry) => Ok(()),
            other => other,
        }
    } else {
        entry.set_password(key)
    };
    result.map_err(|error| Error::Config(format!("store the API key: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dirs(tag: &str) -> AppDirs {
        let root = std::env::temp_dir().join(format!(
            "pstr-meta-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        AppDirs {
            config: root.clone(),
            data: root.clone(),
            cache: root,
        }
    }

    #[test]
    fn settings_round_trip_and_default_to_off() {
        let dirs = temp_dirs("settings");
        assert_eq!(load(&dirs).expect("load"), MetadataConfig::default());
        assert!(!load(&dirs).expect("load").enabled);

        let config = MetadataConfig {
            enabled: true,
            provider: ProviderId::Tmdb,
            language: "de".into(),
        };
        save(&dirs, &config).expect("save");
        assert_eq!(load(&dirs).expect("load"), config);

        std::fs::remove_dir_all(&dirs.config).ok();
    }

    /// The key is a credential and must not be in the settings file, however
    /// convenient that would be.
    #[test]
    fn the_settings_file_never_contains_an_api_key() {
        let dirs = temp_dirs("nokey");
        save(
            &dirs,
            &MetadataConfig {
                enabled: true,
                provider: ProviderId::Tmdb,
                language: "en".into(),
            },
        )
        .expect("save");

        let written = std::fs::read_to_string(settings_file(&dirs)).expect("read");
        assert!(!written.contains("key"), "{written}");

        std::fs::remove_dir_all(&dirs.config).ok();
    }

    /// AniList has no key to look up, and asking for one must not reach the
    /// credential store at all.
    #[test]
    fn a_provider_that_needs_no_key_never_has_one() {
        assert_eq!(api_key(ProviderId::AniList), None);
    }
}
