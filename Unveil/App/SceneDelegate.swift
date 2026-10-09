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
            frames    : engine.frames,
            device    : device
        )

        return UINavigationController(rootViewController: editor)
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
