import AVFoundation
import SwiftUI
import WheelKit

/// First run: scan the pairing code the Mac's Settings › Devices shows, or paste the same JSON.
struct PairView: View {
    let model: PhoneModel
    @State private var pasted = ""
    @State private var problem: String?
    @State private var scanning = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Button("Scan the code on your Mac", systemImage: "qrcode.viewfinder") { scanning = true }
                } footer: {
                    Text("On the Mac, open Settings › Devices.")
                }
                Section("Or paste the pairing code") {
                    TextEditor(text: $pasted)
                        .font(.system(.footnote, design: .monospaced))
                        .frame(minHeight: 110)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    Button("Pair") { submit(pasted) }
                        .disabled(pasted.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                if let message {
                    Section { Label(message, systemImage: "exclamationmark.triangle").foregroundStyle(.red) }
                }
            }
            .navigationTitle("Pair with a Mac")
            .sheet(isPresented: $scanning) {
                QRScanner { code in
                    scanning = false
                    submit(code)
                }
                .ignoresSafeArea()
            }
        }
    }

    private var message: String? {
        if model.certificateMismatch { return "This Mac's certificate does not match" }
        if let problem { return problem }
        if case let .unreachable(why) = model.connection, !model.isPaired, pasted.isEmpty == false { return "Cannot reach the Mac: \(why)" }
        return nil
    }

    private func submit(_ text: String) {
        problem = model.pair(json: text)
    }
}

/// A camera preview that reports the first QR code it reads.
struct QRScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()
        controller.onCode = onCode
        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}
}

final class ScannerController: UIViewController, @preconcurrency AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var done = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: configure()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { ok in
                DispatchQueue.main.async { if ok { self.configure() } else { self.say("Camera access is off. Paste the code instead.") } }
            }
        default: say("Camera access is off. Paste the code instead.")
        }
    }

    private func configure() {
        guard let device = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return say("No camera here. Paste the code instead.") }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.frame = view.bounds
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        nonisolated(unsafe) let s = session; DispatchQueue.global().async { s.startRunning() }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        view.layer.sublayers?.forEach { $0.frame = view.bounds }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        nonisolated(unsafe) let s = session; DispatchQueue.global().async { s.stopRunning() }
    }

    private func say(_ text: String) {
        let label = UILabel()
        label.text = text
        label.textColor = .white
        label.numberOfLines = 0
        label.textAlignment = .center
        label.frame = view.bounds.insetBy(dx: 24, dy: 0)
        label.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(label)
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !done, let code = (objects.first as? AVMetadataMachineReadableCodeObject)?.stringValue else { return }
        done = true
        onCode?(code)
    }
}
