import SwiftUI
import UIKit

struct SharesView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    /// A link opened in this app: the Add form opens with it filled in.
    @Binding var incomingLink: String?

    @State private var showAdd = false
    @State private var signingIn = false
    @State private var browsing = false
    @State private var repairing: ShareRecord?
    @State private var removing: ShareRecord?

    var body: some View {
        TabPage("Shares") {
            ZStack(alignment: .bottomTrailing) {
                if model.shares.isEmpty && model.account.state == nil {
                    EmptyState("No shares yet", "Add a Proton Drive public link to build your library.")
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            AccountCard(onSignIn: { signingIn = true }, onBrowse: { browsing = true })
                            if model.shares.isEmpty {
                                Text("No shares yet. Add a public link, or a folder of your Drive once signed in.")
                                    .textStyle(.bodyMedium)
                                    .foregroundStyle(scheme.onSurfaceVariant)
                                    .padding(16)
                            }
                            ForEach(model.shares, id: \.id) { share in
                                ShareRow(
                                    share: share,
                                    onRefresh: { model.refreshShare(share.id) },
                                    onRepair: { repairing = share },
                                    onRemove: { removing = share }
                                )
                            }
                        }
                        .padding(.top, 8)
                        .padding(.bottom, 96)
                    }
                }
                // Where a thumb is, and the one thing this page is for.
                Button { showAdd = true } label: {
                    Label("Add share", systemImage: "plus")
                        .textStyle(.labelLarge)
                        .padding(.horizontal, 20)
                        .frame(height: 56)
                        .foregroundStyle(scheme.onPrimary)
                        .background(scheme.primary, in: RoundedRectangle(cornerRadius: 16))
                        .shadow(color: .black.opacity(0.3), radius: 6, y: 3)
                }
                .buttonStyle(.plain)
                .padding(16)
            }
        }
        .sheet(isPresented: Binding(get: { showAdd || incomingLink != nil }, set: { if !$0 { showAdd = false; incomingLink = nil } })) {
            AddShareSheet(initialUrl: incomingLink ?? "") { name, url, password in
                model.addShare(name: name, url: url, password: password)
            }
        }
        .sheet(isPresented: Binding(
            get: { signingIn || model.account.state == .secondFactor || model.account.state == .mailboxPassword },
            set: { shown in
                if shown { return }
                signingIn = false
                // Swiped away mid sign-in: the half-made session is spent.
                if model.account.state == .secondFactor || model.account.state == .mailboxPassword { model.cancelSignIn() }
            }
        )) {
            SignInSheet { signingIn = false }
        }
        .fullScreenCover(isPresented: $browsing) {
            DriveBrowser()
        }
        .sheet(item: $repairing) { share in
            RepairShareSheet(share: share) { url, password in
                model.repairShare(id: share.id, url: url, password: password)
            }
        }
        // Asked, because it cannot be undone: the link's secret and its
        // offline files go. Watch positions stay (`Catalog::remove_share`).
        .alert(
            removing.map { "Remove \($0.name)?" } ?? "",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            presenting: removing
        ) { share in
            Button("Remove", role: .destructive) { model.removeShare(share.id) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Its titles leave the library and its offline episodes are deleted. Watch progress is kept, so adding the link again picks up where you left off.")
        }
    }
}

extension ShareRecord: Identifiable {}

/// One share: its name, how it is unlocked, and its actions behind a menu.
private struct ShareRow: View {
    let share: ShareRecord
    let onRefresh: () -> Void
    let onRepair: () -> Void
    let onRemove: () -> Void
    @Environment(\.scheme) private var scheme

    var body: some View {
        HStack(spacing: 16) {
            Image(systemName: "folder.badge.person.crop").font(.system(size: 20)).foregroundStyle(scheme.primary).frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(share.name).textStyle(.bodyLarge, semibold: true).foregroundStyle(scheme.onSurface)
                Text(share.fromAccount ? "Folder of your Drive"
                    : share.hasCustomPassword ? "Link and custom password, stored securely" : "Public link")
                    .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
            }
            Spacer(minLength: 0)
            Menu {
                // One share, not the library: a link that has just had files
                // added should not cost a walk of every other one.
                Button(action: onRefresh) { Label("Refresh", systemImage: "arrow.clockwise") }
                // Not a remove: a share whose secret has become unreadable
                // cannot be removed either.
                if !share.fromAccount {
                    Button(action: onRepair) { Label("Re-enter link", systemImage: "key") }
                }
                Button(role: .destructive, action: onRemove) { Label("Remove", systemImage: "trash") }
            } label: {
                Image(systemName: "ellipsis").font(.system(size: 20)).frame(width: 48, height: 48)
                    .foregroundStyle(scheme.onSurfaceVariant)
            }
            .accessibilityLabel("Actions for \(share.name)")
        }
        .padding(.leading, 16)
        .padding(.trailing, 4)
        .padding(.vertical, 8)
    }
}

/// Re-enter the link for a share the app can no longer decrypt its secret for.
///
/// It has to be the *same* link — a different token is a different share.
/// Rust enforces that; this only says so.
private struct RepairShareSheet: View {
    let share: ShareRecord
    let onRepair: (String, String?) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var url = ""
    @State private var password = ""

    var body: some View {
        DialogSheet("Re-enter link for \(share.name)") {
            Text("The stored credentials for this share cannot be read — usually after a device restore or a Keychain reset. Entering the same link again restores access without losing the library or anything downloaded.")
                .textStyle(.bodySmall)
            Field(label: "Public share URL", text: $url, secret: true)
            Field(label: "Custom password (optional)", text: $password, secret: true)
        } buttons: {
            Button("Cancel") { dismiss() }.buttonStyle(.tonal)
            Button("Restore") {
                onRepair(url.trimmingCharacters(in: .whitespaces), password)
                dismiss()
            }
            .buttonStyle(.accent)
            .disabled(url.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .secureContent()
    }
}

private struct AddShareSheet: View {
    let onAdd: (String, String, String?) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var url: String
    @State private var password = ""

    init(initialUrl: String, onAdd: @escaping (String, String, String?) -> Void) {
        self.onAdd = onAdd
        _url = State(initialValue: initialUrl)
    }

    var body: some View {
        DialogSheet("Add Proton Drive share") {
            Field(label: "Library name", text: $name)
            // The URL fragment is a secret, so the field is a secure one: no
            // keyboard learning, no suggestions, and paste still works.
            Field(label: "Public share URL", text: $url, secret: true)
            // There is no share sheet into a sideloaded app, so a link copied
            // elsewhere comes in from the clipboard.
            if url.isEmpty {
                Button {
                    if let text = UIPasteboard.general.string, let link = shareLink(in: text) { url = link }
                } label: {
                    Label("Paste link", systemImage: "doc.on.clipboard")
                }
                .buttonStyle(.quiet)
            }
            Field(label: "Custom password (optional)", text: $password, secret: true)
        } buttons: {
            Button("Cancel") { dismiss() }.buttonStyle(.tonal)
            Button("Add") {
                onAdd(name.trimmingCharacters(in: .whitespaces), url.trimmingCharacters(in: .whitespaces), password)
                dismiss()
            }
            .buttonStyle(.accent)
            .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || url.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .secureContent()
    }
}
