import SwiftUI
import UIKit

/// Which shares an invite is being made for; empty is the whole library.
struct InviteRequest: Identifiable {
    let ids: [String]
    var id: String { ids.joined(separator: ",") }
}

/// A `pstr://` invite that arrived: what it adds, ticked, and Add. The port
/// of Android's `IncomingInviteDialog`.
///
/// Shares the library already has are shown, unticked and disabled, so a
/// re-sent invite says why it adds less than it lists.
struct IncomingInviteSheet: View {
    let link: String
    let onAdd: ([InvitedShareRecord]) async -> String?
    @Environment(AppModel.self) private var model
    @Environment(\.scheme) private var scheme
    @Environment(\.dismiss) private var dismiss
    @State private var decoded: Result<[InvitedShareRecord], Error>?
    @State private var ticked: [Bool] = []
    @State private var adding = false
    @State private var refusal: String?

    private var shares: [InvitedShareRecord] { (try? decoded?.get()) ?? [] }

    private var chosen: [InvitedShareRecord] {
        shares.enumerated()
            .filter { index, share in !share.alreadyAdded && ticked.indices.contains(index) && ticked[index] }
            .map(\.element)
    }

    var body: some View {
        DialogSheet(shares.count > 1 ? "Add \(shares.count) shared libraries?" : "Add a shared library?") {
            switch decoded {
            case nil:
                ProgressView().frame(maxWidth: .infinity)
            case .failure(let error):
                Text(errorMessage(error)).textStyle(.bodyMedium).foregroundStyle(scheme.error)
            case .success:
                Text("Someone sent you an invite. Each share opens before it is added.")
                    .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                ForEach(Array(shares.enumerated()), id: \.offset) { index, share in
                    Toggle(isOn: Binding(
                        get: { !share.alreadyAdded && ticked.indices.contains(index) && ticked[index] },
                        set: { if ticked.indices.contains(index) { ticked[index] = $0 } }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(share.name.isEmpty ? "Shared folder" : share.name)
                                .textStyle(.bodyLarge).foregroundStyle(scheme.onSurface)
                            if share.alreadyAdded {
                                Text("Already in your library").textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
                            } else if share.customPassword != nil {
                                Text("Password included").textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
                            }
                        }
                    }
                    .tint(scheme.primary)
                    .disabled(share.alreadyAdded || adding)
                }
            }
            if let refusal {
                Text(refusal).textStyle(.bodySmall).foregroundStyle(scheme.error)
            }
        } buttons: {
            Button("Cancel") { dismiss() }.buttonStyle(.tonal).disabled(adding)
            Button {
                adding = true
                Task {
                    let why = await onAdd(chosen)
                    adding = false
                    if let why { refusal = why } else { dismiss() }
                }
            } label: {
                if adding { ProgressView() } else { Text(chosen.count > 1 ? "Add \(chosen.count)" : "Add") }
            }
            .buttonStyle(.accent)
            .disabled(chosen.isEmpty || adding)
        }
        // The invite holds passwords; keep it out of screenshots.
        .secureContent()
        .task(id: link) {
            let result = await model.decodeInvite(link)
            decoded = result
            ticked = ((try? result.get()) ?? []).map { !$0.alreadyAdded }
        }
    }
}

/// An invite to hand out: its QR code, Send and Copy. The link itself is never
/// printed: it carries every password it shares.
struct OutgoingInviteSheet: View {
    let invite: Result<InviteRecord, Error>
    @Environment(\.scheme) private var scheme
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        DialogSheet("Invite to your library") {
            switch invite {
            case .failure(let error):
                Text(errorMessage(error)).textStyle(.bodyMedium).foregroundStyle(scheme.error)
            case .success(let record):
                Text("Scan with a device that has Proton Stream, or send the link.")
                    .textStyle(.bodyMedium).foregroundStyle(scheme.onSurfaceVariant)
                if let qr = try? qrCode(text: record.link) {
                    QrView(qr: qr).frame(width: 220, height: 220).frame(maxWidth: .infinity)
                }
                Text("Anyone with this invite can open what it shares, passwords included. Send it only to people you trust.")
                    .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
                if record.leftOut > 0 {
                    Text("\(record.leftOut) folder(s) of your Proton account left out: they have no link to share.")
                        .textStyle(.bodySmall).foregroundStyle(scheme.onSurfaceVariant)
                }
            }
        } buttons: {
            if case .success(let record) = invite {
                Button("Copy") {
                    // Marked local-only so Universal Clipboard does not carry
                    // the passwords to every other device signed in.
                    UIPasteboard.general.setItems(
                        [[UIPasteboard.typeAutomatic: record.link]],
                        options: [.localOnly: true, .expirationDate: Date.now.addingTimeInterval(600)]
                    )
                    dismiss()
                }
                .buttonStyle(.tonal)
                ShareLink(item: record.link, message: Text("Watch with me on Proton Stream")) {
                    Text("Send")
                }
                .buttonStyle(.accent)
            } else {
                Button("Close") { dismiss() }.buttonStyle(.tonal)
            }
        }
        .secureContent()
    }
}

/// A QR code, black on white whatever the theme, with the four-module quiet
/// zone the standard asks for, every module on whole points.
struct QrView: View {
    let qr: QrRecord

    var body: some View {
        Canvas { context, size in
            let width = Int(qr.width)
            let cells = CGFloat(width + 8)
            let cell = (min(size.width, size.height) / cells).rounded(.down)
            guard cell >= 1 else { return }
            let origin = ((min(size.width, size.height) - cell * CGFloat(width)) / 2).rounded(.down)
            var path = Path()
            for y in 0 ..< width {
                for x in 0 ..< width where qr.dark[y * width + x] {
                    path.addRect(CGRect(x: origin + CGFloat(x) * cell, y: origin + CGFloat(y) * cell, width: cell, height: cell))
                }
            }
            context.fill(path, with: .color(.black))
        }
        .background(Color.white, in: RoundedRectangle(cornerRadius: Corner.small))
        .accessibilityLabel("QR code for an invite link")
    }
}
