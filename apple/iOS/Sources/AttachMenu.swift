import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers
import WheelKit
import WheelUI

/// The composer's + on the phone: Photo Library (several at once), Take Photo where the device has a camera, and Choose
/// File, when wheeld takes uploads; then Clear chat, which asks first, where the harness resets on `/clear`. The model
/// is picked on the composer's row (`ModelMenu`).
/// Every pick lands in the pane's `AttachmentTray`.
struct AttachMenu: View {
    let tray: AttachmentTray
    /// Whether wheeld takes uploads: without, the menu holds Clear chat alone.
    let canAttach: Bool
    /// Starts the conversation over; nil for a harness that does not reset on `/clear`, which gets no Clear chat.
    let clear: (() -> Void)?
    @State private var photosOpen = false
    @State private var cameraOpen = false
    @State private var filesOpen = false
    @State private var confirmClear = false
    @State private var picked: [PhotosPickerItem] = []
    @Environment(\.composerBlur) private var blurComposer

    var body: some View {
        Menu {
            Group { items }
                // A menu draws under the keyboard's window, so opening it drops the keyboard (wheel-eg4e.19.40).
                .onAppear { blurComposer() }
        } label: {
            Label("Attach", systemImage: "plus").labelStyle(.iconOnly)
        }
        .confirmationDialog("Clear this chat?", isPresented: $confirmClear, titleVisibility: .visible) {
            Button("Clear chat", role: .destructive) { clear?() }
                .accessibilityIdentifier("confirm clear chat")
        } message: {
            Text("The agent starts a new conversation.")
        }
        // `.compatible` has Photos hand over a HEIC as JPEG, the form an agent can read.
        .photosPicker(isPresented: $photosOpen, selection: $picked, maxSelectionCount: 10, matching: .images,
                      preferredItemEncoding: .compatible)
        .onChange(of: picked) { _, items in
            guard !items.isEmpty else { return }
            picked = []
            Task { await load(items) }
        }
        .fileImporter(isPresented: $filesOpen, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            switch result {
            case let .success(urls): for url in urls { AttachmentMedia.addFile(url, to: tray) }
            case let .failure(error): Log.info("file picker failed: \(error.localizedDescription)")
            }
        }
        .fullScreenCover(isPresented: $cameraOpen) {
            CameraPicker { image in AttachmentMedia.addPhoto(image, to: tray) }
                .ignoresSafeArea()
        }
    }

    @ViewBuilder private var items: some View {
        if canAttach {
            Button("Photo Library", systemImage: "photo.on.rectangle") { photosOpen = true }
            if UIImagePickerController.isSourceTypeAvailable(.camera) {
                Button("Take Photo", systemImage: "camera") { cameraOpen = true }
            }
            Button("Choose File", systemImage: "folder") { filesOpen = true }
            Divider()
        }
        if clear != nil {
            Button("Clear chat", systemImage: "trash") { confirmClear = true }
        }
    }

    /// Read each picked photo in order and add it.
    private func load(_ items: [PhotosPickerItem]) async {
        let stamp = AttachmentMedia.stamp()
        for (n, item) in items.enumerated() {
            let type = item.supportedContentTypes.first
            let ext = type?.preferredFilenameExtension ?? "jpg"
            let name = items.count == 1 ? "Photo-\(stamp).\(ext)" : "Photo-\(stamp)-\(n + 1).\(ext)"
            do {
                guard let data = try await item.loadTransferable(type: Data.self) else {
                    tray.addUnreadable(name: name, why: "Photos gave no data")
                    continue
                }
                let ready = AttachmentMedia.prepared(data, name: name, type: type)
                tray.add(ready.data, name: ready.name, isImage: ready.isImage)
            } catch {
                tray.addUnreadable(name: name, why: error.localizedDescription)
            }
        }
    }
}

/// The system camera, for one photo. SwiftUI has no camera picker of its own.
private struct CameraPicker: UIViewControllerRepresentable {
    let took: (UIImage) -> Void
    @Environment(\.dismiss) private var dismiss

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_: UIImagePickerController, context _: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let parent: CameraPicker
        init(_ parent: CameraPicker) { self.parent = parent }

        func imagePickerController(_: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            if let image = info[.originalImage] as? UIImage { parent.took(image) }
            parent.dismiss()
        }

        func imagePickerControllerDidCancel(_: UIImagePickerController) {
            parent.dismiss()
        }
    }
}
