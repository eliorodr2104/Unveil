//
//  AppDelegate.swift
//  Unveil
//

import UIKit

/// AppDelegate is the process entry point and the composition root of the services.
///
/// It opens the one engine session at launch and keeps it, with the memory monitor and the
/// lifecycle observer, for the life of the process. The library is file-locked, so a second session
/// would fail: SceneDelegate takes these instances from here instead of making its own. If the
/// engine does not open, `engineError` says why and the scene shows it instead of the editor.
@main
final class AppDelegate: UIResponder, UIApplicationDelegate {

    /// The engine opens its previews at most this many pixels on the long edge (spec: 2560).
    static let maximumPreviewPixels = 2560

    private(set) var engine     : EngineManager?
    private(set) var engineError: EngineError?

    private var memoryMonitor    : MemoryBudgetMonitor<EngineManager, ProcessAvailableMemory>?
    private var lifecycleObserver: EngineLifecycleObserver<EngineManager>?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        let engine: EngineManager

        do {
            engine = try EngineManager(maxPixels: Self.maximumPreviewPixels)
        } catch {
            engineError = error
            return true
        }

        let memoryMonitor = MemoryBudgetMonitor(
            engine : engine,
            memory : ProcessAvailableMemory()
        )

        let lifecycleObserver = EngineLifecycleObserver(engine: engine)

        memoryMonitor.start()
        lifecycleObserver.start()
        engine.recordState(event: "launch")

        self.engine            = engine
        self.memoryMonitor     = memoryMonitor
        self.lifecycleObserver = lifecycleObserver

        return true
    }
}
