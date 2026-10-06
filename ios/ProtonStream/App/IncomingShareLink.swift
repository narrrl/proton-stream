import Foundation

/// A Proton Drive public link, wherever in a piece of text it sits.
///
/// The fragment is kept: it is the share's secret, and a link without it cannot
/// be opened. Matching stops at whitespace and at the quotes and brackets a
/// message app wraps a link in, which never occur in a link itself.
private let shareLinkPattern = #"https://drive\.proton\.me/urls/[^\s"'<>()\[\]]+"#

/// The first share link in `text`, if any — for the clipboard.
func shareLink(in text: String) -> String? {
    text.range(of: shareLinkPattern, options: .regularExpression).map { String(text[$0]) }
}

/// The share link a URL opened in this app carries, if any.
///
/// iOS hands an app no `https://drive.proton.me` link — Universal Links belong
/// to the domain's owner — so a link arrives through the app's own scheme:
/// `protonstream://add?url=<link>`, or the link itself with its scheme swapped,
/// `protonstream://drive.proton.me/urls/…#…`.
func shareLink(from url: URL) -> String? {
    guard url.scheme?.lowercased() == "protonstream" else { return nil }
    if url.host == "add" {
        let value = URLComponents(url: url, resolvingAgainstBaseURL: false)?
            .queryItems?.first { $0.name == "url" }?.value
        return value.flatMap(wholeShareLink)
    }
    let text = url.absoluteString
    let rest = text[text.index(text.startIndex, offsetBy: "protonstream://".count)...]
    return wholeShareLink("https://\(rest)")
}

/// `value` when the whole of it is a share link — what `ACTION_VIEW` accepts on
/// Android — rather than a link somewhere inside it.
private func wholeShareLink(_ value: String) -> String? {
    guard let range = value.range(of: shareLinkPattern, options: .regularExpression),
          range == value.startIndex ..< value.endIndex
    else { return nil }
    return value
}
