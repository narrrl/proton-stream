import SwiftUI

/// The languages offered by name, as the three-letter tag a Matroska file
/// carries.
///
/// The same short list the desktop names tracks from (`pstr-player`'s
/// `language_name`). mpv treats the two- and three-letter forms of a language
/// as one, so the tag chosen here also matches a file that says "ja". Anything
/// else is still one typed tag away.
let languages: [(code: String, name: String)] = [
    ("jpn", "Japanese"), ("eng", "English"), ("ger", "German"), ("fre", "French"),
    ("spa", "Spanish"), ("ita", "Italian"), ("por", "Portuguese"), ("dut", "Dutch"),
    ("rus", "Russian"), ("pol", "Polish"), ("swe", "Swedish"), ("dan", "Danish"),
    ("nor", "Norwegian"), ("fin", "Finnish"), ("cze", "Czech"), ("hun", "Hungarian"),
    ("tur", "Turkish"), ("ara", "Arabic"), ("heb", "Hebrew"), ("hin", "Hindi"),
    ("kor", "Korean"), ("chi", "Chinese"), ("tha", "Thai"), ("vie", "Vietnamese"),
    ("ukr", "Ukrainian"), ("rum", "Romanian"), ("gre", "Greek"), ("ind", "Indonesian"),
    ("may", "Malay"),
]

/// ISO 639-1, the form a file or the desktop may also have stored.
let twoLetter: [String: String] = [
    "jpn": "ja", "eng": "en", "ger": "de", "fre": "fr", "spa": "es", "ita": "it",
    "por": "pt", "dut": "nl", "rus": "ru", "pol": "pl", "swe": "sv", "dan": "da",
    "nor": "no", "fin": "fi", "cze": "cs", "hun": "hu", "tur": "tr", "ara": "ar",
    "heb": "he", "hin": "hi", "kor": "ko", "chi": "zh", "tha": "th", "vie": "vi",
    "ukr": "uk", "rum": "ro", "gre": "el", "ind": "id", "may": "ms",
]

/// ISO 639-2/T, for the handful whose bibliographic code differs.
private let terminology: [String: String] = [
    "ger": "deu", "fre": "fra", "dut": "nld", "cze": "ces", "chi": "zho", "rum": "ron", "gre": "ell", "may": "msa",
]

/// "Japanese", for a tag in any of the forms a file or the desktop may have
/// stored — "jpn", "ja", "JA-jp" — and the tag itself for one not on the list.
/// Nil is no preference.
func languageLabel(_ tag: String?) -> String {
    guard let value = tag?.trimmingCharacters(in: .whitespaces), !value.isEmpty else { return "No preference" }
    return languageOf(value)?.name ?? value
}

/// The listed language a tag names, matching either ISO 639 form.
private func languageOf(_ tag: String) -> (code: String, name: String)? {
    let base = tag.split(whereSeparator: { $0 == "-" || $0 == "_" }).first.map { $0.lowercased() } ?? ""
    return languages.first { $0.code == base || twoLetter[$0.code] == base || terminology[$0.code] == base }
}

/// A language preference as a row, opening a list to choose from.
struct LanguageSetting: View {
    let headline: String
    let value: String?
    let onChange: (String?) -> Void
    @State private var choosing = false

    var body: some View {
        Button { choosing = true } label: {
            ListRow(headline: headline, supporting: languageLabel(value))
        }
        .buttonStyle(.plain)
        .sheet(isPresented: $choosing) {
            LanguageSheet(headline: headline, value: value) { tag in
                choosing = false
                onChange(tag)
            }
        }
    }
}

private struct LanguageSheet: View {
    let headline: String
    let onChoose: (String?) -> Void
    private let current: String?
    @State private var typed: String
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scheme) private var scheme

    init(headline: String, value: String?, onChoose: @escaping (String?) -> Void) {
        self.headline = headline
        self.onChoose = onChoose
        let current = value.flatMap { languageOf($0)?.code ?? $0 }
        self.current = current
        // A tag that is not on the list starts in the field, so it is shown and
        // can be edited rather than silently replaced.
        _typed = State(initialValue: current.flatMap { tag in languages.contains { $0.code == tag } ? nil : tag } ?? "")
    }

    var body: some View {
        DialogSheet(headline) {
            option("No preference", selected: current == nil) { onChoose(nil) }
            ForEach(languages, id: \.code) { language in
                option(language.name, selected: current == language.code) { onChoose(language.code) }
            }
            Divider().overlay(scheme.outlineVariant).padding(.vertical, 8)
            Field(label: "Another tag", text: $typed)
            Text("As it appears in the file, such as \"tgl\"").textStyle(.bodySmall)
        } buttons: {
            Button("Cancel") { dismiss() }.buttonStyle(.tonal)
            Button("Use tag") { onChoose(typed.trimmingCharacters(in: .whitespaces).lowercased()) }
                .buttonStyle(.accent)
                .disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    private func option(_ name: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 16) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 20))
                    .foregroundStyle(selected ? scheme.primary : scheme.onSurfaceVariant)
                Text(name).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface)
                Spacer()
            }
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
