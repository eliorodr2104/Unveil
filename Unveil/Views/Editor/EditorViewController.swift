//
//  EditorViewController.swift
//  Unveil
//

import Metal
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// EditorViewController is the editor screen: the Metal canvas fills the view, the SwiftUI
/// adjustment panel sits on the trailing side, and the navigation bar carries "Open" and the
/// diagnostics menu.
///
/// The document picker delegate conformance lives in this primary declaration on purpose: the class
/// is generic, and an extension of a generic class cannot add an Objective-C conformance.
///
/// Errors are shown from `updateProperties()`. UIKit tracks the `@Observable` reads made there, so a
/// new `errorMessage` calls it again and the alert appears; its OK button clears the message through
/// `dismissError()`.
final class EditorViewController<Engine: EngineDriving & EngineDiagnosing, Importer: PhotoImporting>: UIViewController,
    UIDocumentPickerDelegate {

    private static var panelWidth: CGFloat { 320 }

    private let viewModel: EditorViewModel<Engine, Importer>
    private let engine   : Engine
    private let canvas   : CanvasView

    private var soakTest: StressSweep<Engine, Importer>?

    init(
        viewModel: EditorViewModel<Engine, Importer>,
        engine   : Engine,
        device   : some MTLDevice
    ) {
        self.viewModel = viewModel
        self.engine    = engine
        self.canvas    = CanvasView(frames: engine.frames, device: device)

        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("EditorViewController is built in code, not from a storyboard")
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        view.backgroundColor = .black

        installCanvas()
        installPanel()
        installBarItems()
    }

    override func updateProperties() {
        super.updateProperties()

        guard let message = viewModel.errorMessage, presentedViewController == nil else { return }

        let alert = UIAlertController(
            title          : "Something went wrong",
            message        : message,
            preferredStyle : .alert
        )

        let dismiss = UIAlertAction(title: "OK", style: .default) { [viewModel] _ in
            viewModel.dismissError()
        }

        alert.addAction(dismiss)

        present(alert, animated: true)
    }

    // MARK: - UIDocumentPickerDelegate

    func documentPicker(
        _ controller           : UIDocumentPickerViewController,
        didPickDocumentsAt urls: [URL]
    ) {
        guard let pickedURL = urls.first else { return }

        Task { await viewModel.open(pickedURL: pickedURL) }
    }

    // MARK: - Layout

    private func installCanvas() {
        canvas.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(canvas)

        NSLayoutConstraint.activate([
            canvas.topAnchor.constraint(equalTo: view.topAnchor),
            canvas.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            canvas.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            canvas.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        ])
    }

    private func installPanel() {
        let panel = UIHostingController(rootView: AdjustmentPanel(viewModel: viewModel))
        panel.view.backgroundColor = .secondarySystemBackground
        panel.view.translatesAutoresizingMaskIntoConstraints = false

        addChild(panel)
        view.addSubview(panel.view)

        NSLayoutConstraint.activate([
            panel.view.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            panel.view.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            panel.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            panel.view.widthAnchor.constraint(equalToConstant: Self.panelWidth),
        ])

        panel.didMove(toParent: self)
    }

    private func installBarItems() {
        let open = UIBarButtonItem(
            title         : "Open",
            primaryAction : UIAction { [weak self] _ in self?.presentDocumentPicker() }
        )

        let diagnosticsMenu = DiagnosticsMenu.make(
            engine            : engine,
            isPhotoOpen       : { [viewModel] in viewModel.isPhotoOpen },
            possibleTearCount : { [canvas] in canvas.possibleTearCount },
            present           : { [weak self] alert in self?.present(alert, animated: true) },
            startSoakTest     : { [weak self] in self?.startSoakTest() }
        )

        let diagnostics = UIBarButtonItem(
            title : "Diagnostics",
            image : UIImage(systemName: "ladybug"),
            menu  : diagnosticsMenu
        )

        navigationItem.leftBarButtonItems = [open, diagnostics]
    }

    /// startSoakTest runs the 10-minute StressSweep on the open photo. It does nothing with no photo
    /// open, or while a sweep already runs, and shows where the CSV went when the sweep ends.
    private func startSoakTest() {
        guard viewModel.isPhotoOpen, soakTest == nil else { return }

        let sweep = StressSweep(viewModel: viewModel, duration: 600) { [weak self] isComplete, csv in
            self?.soakTest = nil

            let title = isComplete ? "Soak test finished" : "Soak test stopped early"
            let alert = UIAlertController(
                title          : title,
                message        : "Memory samples: Documents/measurements/\(csv.lastPathComponent)",
                preferredStyle : .alert
            )

            alert.addAction(UIAlertAction(title: "OK", style: .default))
            self?.present(alert, animated: true)
        }

        soakTest = sweep
        sweep.start()
    }

    /// presentDocumentPicker opens the Files picker on the original file, not a copy: PhotoImporter
    /// makes the app's own copy inside the security scope.
    private func presentDocumentPicker() {
        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes : [.rawImage, .image],
            asCopy                 : false
        )
        picker.delegate = self

        present(picker, animated: true)
    }
}
