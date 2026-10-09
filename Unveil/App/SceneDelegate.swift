//
//  SceneDelegate.swift
//  Unveil
//

import Metal
import UIKit

/// SceneDelegate builds the single window by hand (the app has no storyboard) and puts the editor
/// in it, wired to the engine the app delegate opened at launch.
///
/// The preview size is the canvas's long edge in pixels, capped at the engine's 2560. The canvas
/// fills the screen, so the screen's native long edge is that size, known before any layout.
///
/// It also refreshes the preview when the app becomes active again. EngineLifecycleObserver
/// suspends the engine on resign, so a drag in flight at that moment loses its renders; the
/// observer here is registered after the lifecycle one (which starts at launch), and it defers
/// the refresh to a Task, so the full preview is requested once the engine has resumed.
final class SceneDelegate: UIResponder, UIWindowSceneDelegate {

    var window: UIWindow?

    // The notification token type is an existential declared by Foundation.
    private var becomeActiveToken: (any NSObjectProtocol)?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = scene as? UIWindowScene else { return }

        let window = UIWindow(windowScene: windowScene)
        window.rootViewController = makeRootViewController(for: windowScene)
        window.makeKeyAndVisible()

        self.window = window
    }

    func sceneDidDisconnect(_ scene: UIScene) {
        if let becomeActiveToken {
            NotificationCenter.default.removeObserver(becomeActiveToken)
        }

        becomeActiveToken = nil
    }

    private func makeRootViewController(for windowScene: UIWindowScene) -> UIViewController {
        let appDelegate = UIApplication.shared.delegate as? AppDelegate

        guard let engine = appDelegate?.engine,
              let device = MTLCreateSystemDefaultDevice()
        else {
            let reason = appDelegate?.engineError?.message ?? "This device has no Metal GPU."
            return makeUnavailableViewController(reason: reason)
        }

        let nativeBounds  = windowScene.screen.nativeBounds
        let longEdge      = Int(max(nativeBounds.width, nativeBounds.height))
        let previewPixels = min(longEdge, AppDelegate.maximumPreviewPixels)

        let viewModel = EditorViewModel(
            engine        : engine,
            importer      : PhotoImporter(),
            previewPixels : previewPixels
        )

        refreshPreviewOnBecomeActive(viewModel)

        let editor = EditorViewController(
            viewModel : viewModel,
            engine    : engine,
            device    : device
        )

        runLaunchCommand(engine: engine)
        runMeasurement(
            options   : MeasurementLaunchOptions(arguments: ProcessInfo.processInfo.arguments),
            viewModel : viewModel,
            engine    : engine
        )

        return UINavigationController(rootViewController: editor)
    }

    /// runLaunchCommand runs the diagnostic named by the `-UnveilRun` launch argument, once per
    /// launch, so an agent can drive the device with no taps (`devicectl device process launch`).
    ///
    /// `golden` exports the reference images, prints its progress and ends the process: `exit(0)` when
    /// every image was written, `exit(1)` otherwise. The exit is allowed only in this agent-driven
    /// mode, where nobody is editing. `state` appends one `engine.jsonl` line and stays open.
    private func runLaunchCommand(engine: EngineManager) {
        guard let command = UserDefaults.standard.string(forKey: "UnveilRun") else { return }

        Task {
            // The engine is suspended while the app is not active: wait for the first activation.
            while UIApplication.shared.applicationState != .active {
                try? await Task.sleep(for: .milliseconds(100))
            }

            switch command {
                case "golden":
                    let summary = await GoldenExporter(engine: engine).run()
                    exit(summary.failures.isEmpty ? 0 : 1)

                case "state":
                    engine.recordState(event: "run-state")

                default:
                    print("unknown -UnveilRun value: \(command)")
            }
        }
    }

    /// runMeasurement opens the photo named by `-UnveilOpen` and runs the sweep of `-UnveilSweep`
    /// (see MeasurementLaunchOptions), so T15's runs need no taps. Without `-UnveilOpen` it does
    /// nothing, and a normal launch is unchanged.
    ///
    /// Before the open it waits for an `app.gpu` read on the engine's serial command queue, which
    /// lands behind the launch read (the first one creates the GPU device): so `OpenToFirstFrame`
    /// starts after that cost, and the launch read shows apart as its own `Command` interval.
    ///
    /// With `-UnveilExitAfterSweep` the process ends when the sweep does, `exit(0)` only when it ran
    /// its full time, and `exit(1)` if the photo never showed a full frame. Like `-UnveilRun golden`,
    /// the exit is allowed only in this agent-driven mode, where nobody is editing.
    private func runMeasurement(
        options  : MeasurementLaunchOptions,
        viewModel: EditorViewModel<EngineManager, PhotoImporter>,
        engine   : EngineManager
    ) {
        guard let fileName = options.openFileName else { return }

        Task {
            while UIApplication.shared.applicationState != .active {
                try? await Task.sleep(for: .milliseconds(100))
            }

            try? await Task.sleep(for: .seconds(options.delaySeconds))
            _ = try? await engine.diagnostic("app.gpu", params: [String: String]())

            print("measure: opening \(fileName)")
            await viewModel.open(pickedURL: URL.documentsDirectory.appending(path: "raw/\(fileName)"))

            guard await Self.waitForFullFrame(in: engine.frames) else {
                print("measure: no full frame for \(fileName): \(viewModel.errorMessage ?? "timed out")")
                if options.exitsAfterSweep { exit(1) }
                return
            }

            print("measure: first full frame")
            guard let seconds = options.sweepSeconds else { return }

            let sweep = StressSweep(viewModel: viewModel, duration: seconds) { isComplete, _ in
                if options.exitsAfterSweep { exit(isComplete ? 0 : 1) }
            }

            sweep.start()
        }
    }

    /// waitForFullFrame polls the sink every 20 ms, for at most 60 s, until it holds a full frame.
    /// The sink starts empty at launch, so the first full frame is the open's preview.
    private static func waitForFullFrame(in frames: FrameSink) async -> Bool {
        for _ in 0 ..< 3000 {
            if let frame = frames.latest(), !frame.isDraft { return true }

            try? await Task.sleep(for: .milliseconds(20))
        }

        return false
    }

    /// makeUnavailableViewController is the screen shown when the engine did not open: without it
    /// there is nothing to edit, so the reason is all there is to show.
    private func makeUnavailableViewController(reason: String) -> UIViewController {
        var configuration = UIContentUnavailableConfiguration.empty()
        configuration.image         = UIImage(systemName: "exclamationmark.triangle")
        configuration.text          = "The engine did not start."
        configuration.secondaryText = reason

        let unavailable = UIViewController()
        unavailable.view.backgroundColor            = .systemBackground
        unavailable.contentUnavailableConfiguration = configuration

        return unavailable
    }

    /// refreshPreviewOnBecomeActive asks the editor for one full preview each time the app becomes
    /// active, after the engine has resumed (see the type's doc for the ordering).
    private func refreshPreviewOnBecomeActive(
        _ viewModel: EditorViewModel<EngineManager, PhotoImporter>
    ) {
        becomeActiveToken = NotificationCenter.default.addObserver(
            forName : UIApplication.didBecomeActiveNotification,
            object  : nil,
            queue   : nil
        ) { _ in
            Task { @MainActor in await viewModel.refreshPreview() }
        }
    }
}
