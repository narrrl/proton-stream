//! Language tags: what a container or a file name calls a language, and what a
//! viewer calls it.

/// The languages offered by name where a preference is chosen ahead of any
/// file, as the three-letter tag a Matroska file carries. The same list
/// [`language_name`] knows, and the phones' `LANGUAGES`; mpv treats the two-
/// and three-letter forms as one.
pub const LANGUAGES: [&str; 29] = [
    "jpn", "eng", "ger", "fre", "spa", "ita", "por", "dut", "rus", "pol", "swe", "dan", "nor",
    "fin", "cze", "hun", "tur", "ara", "heb", "hin", "kor", "chi", "tha", "vie", "ukr", "rum",
    "gre", "ind", "may",
];

/// Each language's name and the tags it goes by, the [`LANGUAGES`] form first.
///
/// Deliberately a short list rather than a full ISO 639 table: it covers what
/// the shares this app is pointed at actually contain, and an unrecognised tag
/// still shows as something a viewer can pick between — `"por-BR"` reads fine
/// on its own.
const TABLE: [(&str, &[&str]); 29] = [
    ("English", &["eng", "en"]),
    ("Japanese", &["jpn", "ja"]),
    ("German", &["ger", "de", "deu"]),
    ("French", &["fre", "fr", "fra"]),
    ("Spanish", &["spa", "es"]),
    ("Italian", &["ita", "it"]),
    ("Portuguese", &["por", "pt"]),
    ("Dutch", &["dut", "nl", "nld"]),
    ("Russian", &["rus", "ru"]),
    ("Polish", &["pol", "pl"]),
    ("Swedish", &["swe", "sv"]),
    ("Danish", &["dan", "da"]),
    ("Norwegian", &["nor", "no"]),
    ("Finnish", &["fin", "fi"]),
    ("Czech", &["cze", "cs", "ces"]),
    ("Hungarian", &["hun", "hu"]),
    ("Turkish", &["tur", "tr"]),
    ("Arabic", &["ara", "ar"]),
    ("Hebrew", &["heb", "he"]),
    ("Hindi", &["hin", "hi"]),
    ("Korean", &["kor", "ko"]),
    ("Chinese", &["chi", "zh", "zho"]),
    ("Thai", &["tha", "th"]),
    ("Vietnamese", &["vie", "vi"]),
    ("Ukrainian", &["ukr", "uk"]),
    ("Romanian", &["rum", "ro", "ron"]),
    ("Greek", &["gre", "el", "ell"]),
    ("Indonesian", &["ind", "id"]),
    ("Malay", &["may", "ms", "msa"]),
];

/// A readable name for a language tag, falling back to the tag itself.
pub fn language_name(tag: &str) -> String {
    // Container tags come in both the two- and three-letter forms, and often
    // with a region suffix: "en", "eng", "en-US", "pt-BR".
    let base = tag.split(['-', '_']).next().unwrap_or(tag);
    let Some((name, _)) = TABLE
        .iter()
        .find(|(_, tags)| tags.iter().any(|known| known.eq_ignore_ascii_case(base)))
    else {
        return tag.to_string();
    };

    // The region matters when there is one — Brazilian Portuguese is not
    // Portuguese to anyone choosing between the two.
    match tag.split_once(['-', '_']) {
        Some((_, region)) if !region.is_empty() => {
            format!("{name} ({})", region.to_ascii_uppercase())
        }
        _ => (*name).to_string(),
    }
}

/// Whether two container tags name the same language: "en" and "eng" do.
pub fn same_language(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || language_name(a) == language_name(b)
}

/// The [`LANGUAGES`] tag for one word of a file name, if it names a language:
/// "en", "eng" and "English" are all `"eng"`.
///
/// For the words around a subtitle file's name — `Episode 01.en.srt`,
/// `2_English.srt` — where a two-letter word is far more often a language than
/// anything else.
pub fn language_tag(word: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(name, tags)| {
            name.eq_ignore_ascii_case(word) || tags.iter().any(|tag| tag.eq_ignore_ascii_case(word))
        })
        .map(|(_, tags)| tags[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_resolve_in_both_iso_forms_and_keep_their_region() {
        assert_eq!(language_name("jpn"), "Japanese");
        assert_eq!(language_name("ja"), "Japanese");
        assert_eq!(language_name("pt-BR"), "Portuguese (BR)");
        // An unknown tag is shown as it stands rather than as "Unknown", which
        // would make two of them indistinguishable.
        assert_eq!(language_name("mis"), "mis");
    }

    #[test]
    fn a_language_named_by_either_tag_or_by_its_name_has_the_tag_a_preference_uses() {
        assert_eq!(language_tag("en"), Some("eng"));
        assert_eq!(language_tag("ENG"), Some("eng"));
        assert_eq!(language_tag("German"), Some("ger"));
        assert_eq!(language_tag("deu"), Some("ger"));
        assert_eq!(language_tag("forced"), None);
    }

    #[test]
    fn every_language_offered_by_name_is_one_the_table_knows() {
        let firsts: Vec<&str> = TABLE.iter().map(|(_, tags)| tags[0]).collect();
        let mut offered = LANGUAGES.to_vec();
        let mut known = firsts.clone();
        offered.sort_unstable();
        known.sort_unstable();
        assert_eq!(offered, known);
    }
}
