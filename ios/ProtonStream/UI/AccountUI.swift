import SwiftUI
import WebKit

/// The Proton account, at the top of the shares page: why one would sign in,
/// or who is signed in and how watch-history sync is doing.
struct AccountCard: View {
    let onSignIn: () -> Void
    let onBrowse: () -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme

    var body: some View {
        if let state = model.account.state {
            Group {
                if case let .signedIn(username) = state {
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(spacing: 16) {
                            Image(systemName: "person.crop.circle.fill").font(.system(size: 22)).foregroundStyle(scheme.primary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(username).textStyle(.bodyLarge, semibold: true).foregroundStyle(scheme.onSurface)
                                TimelineView(.periodic(from: .now, by: 30)) { context in
                                    Text(syncLine(model.account, now: context.date))
                                        .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                                }
                            }
                            Spacer(minLength: 0)
                            Menu {
                                Button(action: model.syncNow) { Label("Sync now", systemImage: "arrow.triangle.2.circlepath") }
                                Button(action: model.signOut) { Label("Sign out", systemImage: "rectangle.portrait.and.arrow.right") }
                            } label: {
                                Image(systemName: "ellipsis").font(.system(size: 20)).frame(width: 48, height: 48)
                                    .foregroundStyle(scheme.onSurfaceVariant)
                            }
                            .accessibilityLabel("Account actions")
                        }
                        .padding(.leading, 16)
                        .padding(.trailing, 4)
                        .padding(.vertical, 8)
                        Button(action: onBrowse) { Label("Add from your Drive", systemImage: "folder") }
                            .buttonStyle(.tonal)
                            .padding(.horizontal, 16)
                            .padding(.bottom, 12)
                    }
                } else {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Proton account").textStyle(.titleMedium).foregroundStyle(scheme.onSurface)
                        Text("Optional. Signed in, your shares, settings and watch history are kept in your own Drive, so every device shows the same library and resumes where another left off, and folders of your Drive can join the library.")
                            .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                        Button("Sign in", action: onSignIn).buttonStyle(.tonal)
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .card()
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
    }
}

/// "Synced 3 min ago · 2 from other devices", or why it did not.
func syncLine(_ account: AccountUiState, now: Date) -> String {
    if let error = account.syncError { return "Did not sync: \(error)" }
    guard let at = account.syncedAt else { return "Library, settings and watch history sync through your Drive" }
    let minutes = Int(max(now.timeIntervalSince(at), 0) / 60)
    let ago = minutes < 1 ? "just now" : minutes < 60 ? "\(minutes) min ago" : "\(minutes / 60) h ago"
    let applied = account.applied > 0 ? " · \(account.applied) from other devices" : ""
    return "Synced \(ago)\(applied)"
}

/// Security keys need WebAuthn, which the sign-in here does not speak. An
/// account with nothing else is refused before this form (`AccountStore::sign_in`).
private let securityKeyNote = "Security keys are not supported here — use the code from your authenticator app."

/// Signing in, one step at a time: address and password, then a second-factor
/// code or the mailbox password if the account has them, with Proton's CAPTCHA
/// over the top when it asks for one.
struct SignInSheet: View {
    let onDismiss: () -> Void
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @State private var username = ""
    @State private var password = ""
    @State private var code = ""
    @State private var mailbox = ""

    var body: some View {
        let account = model.account
        DialogSheet("Sign in to Proton") {
            switch account.state {
            case .secondFactor:
                Field(label: "Code from your authenticator app", text: $code, keyboard: .numberPad, content: .oneTimeCode)
                Text(securityKeyNote).textStyle(.bodySmall)
            case .mailboxPassword:
                Text("This account has a separate mailbox password. It unlocks your files.").textStyle(.bodySmall)
                Field(label: "Mailbox password", text: $mailbox, secret: true, content: .password)
            default:
                Field(label: "Address", text: $username, keyboard: .emailAddress, content: .username)
                Field(label: "Password", text: $password, secret: true, content: .password)
            }
            if let error = account.error {
                Text(error).foregroundStyle(scheme.error)
            }
            if account.busy {
                ProgressView().tint(scheme.primary).padding(.top, 4)
            }
        } buttons: {
            Button("Cancel", action: dismiss).buttonStyle(.tonal)
            switch account.state {
            case .secondFactor:
                Button("Continue") { model.submitSecondFactor(code.trimmingCharacters(in: .whitespaces)) }
                    .buttonStyle(.accent)
                    .disabled(code.trimmingCharacters(in: .whitespaces).isEmpty || account.busy)
            case .mailboxPassword:
                Button("Unlock") { model.submitMailboxPassword(mailbox) }
                    .buttonStyle(.accent)
                    .disabled(mailbox.isEmpty || account.busy)
            default:
                Button("Sign in") { model.signIn(username.trimmingCharacters(in: .whitespaces), password) }
                    .buttonStyle(.accent)
                    .disabled(username.trimmingCharacters(in: .whitespaces).isEmpty || password.isEmpty || account.busy)
            }
        }
        .secureContent()
        .interactiveDismissDisabled(account.busy)
        .onChange(of: account.state) { _, state in
            if case .signedIn = state { onDismiss() }
        }
        .fullScreenCover(isPresented: Binding(
            get: { account.verificationUrl != nil },
            set: { if !$0 { model.dismissVerification() } }
        )) {
            if let url = account.verificationUrl {
                VerificationView(url: url, onToken: { token in
                    model.signIn(username.trimmingCharacters(in: .whitespaces), password, verificationToken: token)
                }, onDismiss: model.dismissVerification)
            }
        }
    }

    private func dismiss() {
        if model.account.state == .secondFactor || model.account.state == .mailboxPassword { model.cancelSignIn() }
        onDismiss()
    }
}

/// Proton's hosted CAPTCHA, which it puts in front of a sign-in it does not
/// recognise — a new network, a VPN exit.
///
/// In a web view rather than Safari because the page reports success by
/// posting a message to its host, not by redirecting. The page targets
/// `window.parent`, which for a top-level page is the page itself, so a
/// listener on its own window hears it.
private struct VerificationView: View {
    let url: String
    let onToken: (String) -> Void
    let onDismiss: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Confirm you are human").textStyle(.titleMedium).foregroundStyle(scheme.onSurface).padding(.leading, 16)
                Spacer()
                IconButton("xmark", "Cancel", action: onDismiss)
            }
            .frame(height: 56)
            VerificationWebView(url: url, onToken: onToken)
        }
        .background(scheme.surfaceContainerHigh)
    }
}

/// Installed once per page, at start and again at finish in case the first ran
/// too early.
private let verificationBridge = """
if (!window.__pstrVerification) {
    window.__pstrVerification = true;
    window.addEventListener('message', function (event) {
        try {
            window.webkit.messageHandlers.pstrVerification.postMessage(
                typeof event.data === 'string' ? event.data : JSON.stringify(event.data));
        } catch (e) {}
    });
}
"""

/// Whether a page is Proton's own, over HTTPS.
func isProtonPage(scheme: String?, host: String?) -> Bool {
    guard scheme == "https", let host else { return false }
    return host == "proton.me" || host.hasSuffix(".proton.me")
}

private struct VerificationWebView: UIViewRepresentable {
    let url: String
    let onToken: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onToken: onToken) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let content = configuration.userContentController
        for time in [WKUserScriptInjectionTime.atDocumentStart, .atDocumentEnd] {
            content.addUserScript(WKUserScript(source: verificationBridge, injectionTime: time, forMainFrameOnly: true))
        }
        content.add(WeakHandler(context.coordinator), name: "pstrVerification")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        if let address = URL(string: url) { view.load(URLRequest(url: address)) }
        return view
    }

    func updateUIView(_: WKWebView, context _: Context) {}

    static func dismantleUIView(_ view: WKWebView, coordinator _: Coordinator) {
        view.configuration.userContentController.removeScriptMessageHandler(forName: "pstrVerification")
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
        let onToken: (String) -> Void
        private var answered = false

        init(onToken: @escaping (String) -> Void) {
            self.onToken = onToken
        }

        /// Every message the page posts lands here, most of them its own chatter.
        func userContentController(_: WKUserContentController, didReceive message: WKScriptMessage) {
            guard !answered, let text = message.body as? String, let token = verificationToken(message: text) else { return }
            answered = true
            onToken(token)
        }

        /// The bridge is reachable by whatever the frame shows, so the frame
        /// stays on Proton.
        func webView(_: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
            guard action.targetFrame?.isMainFrame ?? true else { return .allow }
            return isProtonPage(scheme: action.request.url?.scheme, host: action.request.url?.host) ? .allow : .cancel
        }
    }

    /// `WKUserContentController` holds its handlers strongly.
    @MainActor
    private final class WeakHandler: NSObject, WKScriptMessageHandler {
        weak var target: WKScriptMessageHandler?

        init(_ target: WKScriptMessageHandler) {
            self.target = target
        }

        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            target?.userContentController(controller, didReceive: message)
        }
    }
}

/// Walk the account's Drive and add a folder of it to the library.
///
/// Full screen, because a Drive is deep. Listings are asked for here rather
/// than through the model: they are this view's alone, and gone when it closes.
struct DriveBrowser: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @Environment(\.dismiss) private var dismiss
    @State private var trail: [(name: String, place: DrivePlaceRecord)] = []
    @State private var places: [DrivePlaceRecord]?
    @State private var entries: [DriveEntryRecord]?
    @State private var error: String?

    var body: some View {
        let current = trail.last?.place
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                IconButton(trail.isEmpty ? "xmark" : "chevron.backward", trail.isEmpty ? "Close" : "Up a folder", tint: scheme.onSurface) {
                    if trail.isEmpty { dismiss() } else { trail.removeLast() }
                }
                Text(trail.last?.name ?? "Your Drive").textStyle(.titleLarge).foregroundStyle(scheme.onSurface).lineLimit(1)
                Spacer()
            }
            .frame(height: 56)
            Group {
                let loading = error == nil && (current == nil ? places == nil : entries == nil)
                if let error {
                    EmptyState("Could not list this", error)
                } else if loading {
                    ProgressView().tint(scheme.primary).frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if current == nil {
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(places ?? [], id: \.linkId) { place in
                                DriveRow(icon: place.kind.icon, name: place.name, detail: nil) {
                                    trail.append((place.name, place))
                                }
                            }
                        }
                        .padding(.vertical, 8)
                    }
                } else if (entries ?? []).isEmpty {
                    EmptyState("Empty folder", "Nothing to add from here.")
                } else {
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(entries ?? [], id: \.linkId) { entry in
                                DriveRow(
                                    icon: entry.isFolder ? "folder.fill" : entry.isVideo ? "film" : "doc",
                                    name: entry.name,
                                    detail: entry.size.map(formatBytes),
                                    open: entry.isFolder ? {
                                        trail.append((entry.name, DrivePlaceRecord(kind: .myFiles, name: entry.name, volumeId: entry.volumeId, linkId: entry.linkId)))
                                    } : nil
                                )
                            }
                        }
                        .padding(.vertical, 8)
                    }
                }
            }
            .frame(maxHeight: .infinity)
            // A folder already in the library is refused by the engine, with a
            // message saying so.
            if let folder = current, let name = trail.last?.name {
                HStack {
                    Spacer()
                    Button("Add this folder") {
                        model.addAccountFolder(name: name, volumeId: folder.volumeId, linkId: folder.linkId)
                        dismiss()
                    }
                    .buttonStyle(.accent)
                }
                .padding(16)
            }
        }
        .background(scheme.background)
        .environment(\.colorScheme, scheme.colorScheme)
        .task(id: current?.linkId) {
            error = nil
            do {
                if let current {
                    entries = nil
                    entries = try await NativeRuntime.engine().driveFolder(volumeId: current.volumeId, linkId: current.linkId)
                } else if places == nil {
                    places = try await NativeRuntime.engine().drivePlaces()
                }
            } catch {
                self.error = errorMessage(error)
            }
        }
    }
}

private extension PlaceType {
    var icon: String {
        switch self {
        case .myFiles: "externaldrive"
        case .device: "desktopcomputer"
        case .sharedWithMe: "folder.badge.person.crop"
        }
    }
}

private struct DriveRow: View {
    let icon: String
    let name: String
    let detail: String?
    var open: (() -> Void)?
    @Environment(\.scheme) private var scheme

    var body: some View {
        Button { open?() } label: {
            HStack(spacing: 16) {
                Image(systemName: icon).font(.system(size: 20))
                    .foregroundStyle(open != nil ? scheme.primary : scheme.onSurfaceVariant).frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).textStyle(.bodyLarge).foregroundStyle(scheme.onSurface).lineLimit(2)
                    if let detail { Text(detail).textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant) }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(open == nil)
    }
}
