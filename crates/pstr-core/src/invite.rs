//! `pstr://` invites: one link that adds one share, or a whole library, on
//! another device.
//!
//! A share is a Proton Drive public link plus, sometimes, a custom password.
//! Handing both to a friend used to take two messages and a trip through the
//! Add dialog per share. An invite carries them together:
//!
//! ```text
//! pstr://add?v=1&s=<base64url(JSON)>
//! ```
//!
//! where the JSON is a list of `{"n": name, "u": url, "p": password}`, `p`
//! left out when the link has none. Short keys, no padding: the link is also
//! shown as a QR code, and every byte makes the code denser.
//!
//! **An invite is a secret.** It holds each link's `#fragment` and custom
//! password in the clear — base64 is packaging, not protection. Whoever has
//! the invite has the library, exactly as whoever has the share link does;
//! the front ends say so where the link is copied.
//!
//! Folders of a signed-in account have no shareable secret — they open
//! through the account's session — so they are left out of an invite, and the
//! caller is told how many were.

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::shares::share_token;

/// The URL scheme an invite uses, and the one each platform registers.
pub const SCHEME: &str = "pstr";

/// The invite format this build writes. A reader refuses a newer one rather
/// than guessing at fields it does not know.
pub const VERSION: u32 = 1;

/// The most shares one invite may carry. Far past any library anyone shares,
/// and a bound on what a hostile link can make the confirmation sheet list.
pub const MAX_SHARES: usize = 200;

/// The longest name kept from an invite, in characters.
const MAX_NAME: usize = 120;

/// One share an invite adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitedShare {
    /// What the sender calls it. May be empty: the receiver then takes the
    /// shared folder's own name, as the Add dialog does.
    pub name: String,
    /// The full share URL, `#fragment` included.
    pub url: String,
    /// The custom password, when the link has one.
    pub password: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    #[serde(rename = "n", default)]
    name: String,
    #[serde(rename = "u")]
    url: String,
    #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

/// The invite link for `shares`.
///
/// Every URL is checked first, so a link this returns always decodes.
pub fn encode(shares: &[InvitedShare]) -> Result<String> {
    if shares.is_empty() {
        return Err(Error::Config("nothing to invite to".to_owned()));
    }
    if shares.len() > MAX_SHARES {
        return Err(Error::Config(format!(
            "an invite holds at most {MAX_SHARES} shares"
        )));
    }
    let wire = shares
        .iter()
        .map(|share| {
            share_token(&share.url)?;
            Ok(Wire {
                name: share.name.trim().to_owned(),
                url: share.url.clone(),
                password: share
                    .password
                    .clone()
                    .filter(|password| !password.is_empty()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let json = serde_json::to_vec(&wire).map_err(|error| Error::Config(error.to_string()))?;
    Ok(format!(
        "{SCHEME}://add?v={VERSION}&s={}",
        URL_SAFE_NO_PAD.encode(json)
    ))
}

/// The shares `text` adds: a `pstr://` invite, or a bare Proton Drive share
/// link (which adds one share and asks for no name).
///
/// `text` is trimmed and must be the link and nothing else; use [`find`] to
/// pick one out of a message. Duplicate links are dropped, first one kept. No
/// error repeats the link, whose fragment is a password.
pub fn decode(text: &str) -> Result<Vec<InvitedShare>> {
    let text = text.trim();
    if text.starts_with("https://") {
        share_token(text)?;
        return Ok(vec![InvitedShare {
            name: String::new(),
            url: text.to_owned(),
            password: None,
        }]);
    }
    let rest = strip_prefix_ignore_case(text, &format!("{SCHEME}://"))
        .ok_or_else(|| Error::Config("not an invite link".to_owned()))?;
    let query = strip_prefix_ignore_case(rest, "add?")
        .or_else(|| strip_prefix_ignore_case(rest, "add/?"))
        .ok_or_else(|| Error::Config("not an invite link".to_owned()))?;

    let mut version = None;
    let mut payload = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("v", value)) => version = value.parse::<u32>().ok(),
            Some(("s", value)) => payload = Some(value),
            _ => {}
        }
    }
    match version {
        Some(VERSION) => {}
        Some(newer) if newer > VERSION => {
            return Err(Error::Config(
                "this invite was made by a newer version of Proton Stream; update to open it"
                    .to_owned(),
            ));
        }
        _ => return Err(Error::Config("the invite link is damaged".to_owned())),
    }
    let damaged = || Error::Config("the invite link is damaged".to_owned());
    let payload = payload.ok_or_else(damaged)?;
    // A messenger may have added the padding back, or percent-encoded it.
    let payload = payload.trim_end_matches("%3D").trim_end_matches('=');
    let json = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .map_err(|_| damaged())?;
    let wire: Vec<Wire> = serde_json::from_slice(&json).map_err(|_| damaged())?;
    if wire.is_empty() {
        return Err(damaged());
    }
    if wire.len() > MAX_SHARES {
        return Err(Error::Config(format!(
            "an invite holds at most {MAX_SHARES} shares"
        )));
    }

    let mut seen = std::collections::HashSet::new();
    let mut shares = Vec::with_capacity(wire.len());
    for entry in wire {
        let token = share_token(&entry.url)?;
        if !seen.insert(token) {
            continue;
        }
        shares.push(InvitedShare {
            name: entry
                .name
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_NAME)
                .collect(),
            url: entry.url,
            password: entry.password.filter(|password| !password.is_empty()),
        });
    }
    Ok(shares)
}

/// The first invite link in `text` — a message, the clipboard — if any.
///
/// Stops at whitespace and at the quotes and brackets a messenger wraps a link
/// in, none of which a base64url payload contains.
pub fn find(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find(&format!("{SCHEME}://add"))?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Whether `text` is an invite link rather than a bare share link.
pub fn is_invite(text: &str) -> bool {
    strip_prefix_ignore_case(text.trim(), &format!("{SCHEME}://")).is_some()
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK_A: &str = "https://drive.proton.me/urls/ABCDEFGHIJ#fragmentA123";
    const LINK_B: &str = "https://drive.proton.me/urls/KLMNOPQRST#fragmentB456";

    fn share(name: &str, url: &str, password: Option<&str>) -> InvitedShare {
        InvitedShare {
            name: name.to_owned(),
            url: url.to_owned(),
            password: password.map(str::to_owned),
        }
    }

    #[test]
    fn an_invite_for_one_share_decodes_to_that_share() {
        let shares = vec![share("Anime", LINK_A, Some("hunter2"))];
        let link = encode(&shares).unwrap();
        assert!(link.starts_with("pstr://add?v=1&s="));
        assert_eq!(decode(&link).unwrap(), shares);
    }

    #[test]
    fn an_invite_for_a_whole_library_keeps_every_share_in_order() {
        let shares = vec![
            share("Anime", LINK_A, None),
            share("Films", LINK_B, Some("pässwörd")),
        ];
        assert_eq!(decode(&encode(&shares).unwrap()).unwrap(), shares);
    }

    #[test]
    fn an_invite_link_is_safe_to_put_in_a_url_and_a_qr_code() {
        let link = encode(&[share(
            "A name with spaces & ampersands",
            LINK_A,
            Some("p/w+?"),
        )])
        .unwrap();
        let payload = link.split("s=").nth(1).unwrap();
        assert!(
            payload
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        crate::handoff::Qr::encode(&link).unwrap();
    }

    #[test]
    fn a_bare_share_link_is_accepted_as_an_invite_for_one_share() {
        assert_eq!(decode(LINK_A).unwrap(), vec![share("", LINK_A, None)]);
    }

    #[test]
    fn a_share_link_without_its_fragment_is_refused_on_both_sides() {
        let broken = "https://drive.proton.me/urls/ABCDEFGHIJ";
        assert!(encode(&[share("x", broken, None)]).is_err());
        assert!(decode(broken).is_err());
    }

    #[test]
    fn an_invite_from_a_newer_version_asks_for_an_update() {
        let error = decode("pstr://add?v=2&s=e30").unwrap_err().to_string();
        assert!(error.contains("newer version"), "{error}");
    }

    #[test]
    fn a_damaged_payload_is_refused_without_repeating_the_link() {
        let error = decode("pstr://add?v=1&s=%%%secret%%%")
            .unwrap_err()
            .to_string();
        assert!(error.contains("damaged"), "{error}");
        assert!(!error.contains("secret"));
    }

    #[test]
    fn a_repeated_share_in_an_invite_is_added_once() {
        let link = encode(&[share("One", LINK_A, None), share("Two", LINK_A, None)]).unwrap();
        let shares = decode(&link).unwrap();
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].name, "One");
    }

    #[test]
    fn the_scheme_is_matched_without_regard_to_case_and_padding_is_tolerated() {
        let link = encode(&[share("Anime", LINK_A, None)]).unwrap();
        let shouted = link.replacen("pstr://add", "PSTR://ADD", 1);
        assert_eq!(decode(&shouted).unwrap().len(), 1);
        assert_eq!(decode(&format!("{link}==")).unwrap().len(), 1);
    }

    #[test]
    fn an_invite_is_found_inside_a_message() {
        let link = encode(&[share("Anime", LINK_A, None)]).unwrap();
        let message = format!("here you go: \"{link}\" enjoy");
        assert_eq!(find(&message), Some(link.as_str()));
        assert_eq!(find("no link here"), None);
    }

    #[test]
    fn control_characters_in_a_name_are_dropped() {
        let link = encode(&[share("Bad\u{7}\nName", LINK_A, None)]).unwrap();
        assert_eq!(decode(&link).unwrap()[0].name, "BadName");
    }
}
