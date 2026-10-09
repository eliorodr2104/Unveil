//
//  EngineLifecycleObserver.swift
//  Unveil
//

import UIKit
import os

/// EngineLifecycleObserver holds the engine still while the app is not active.
///
/// `willResignActive` suspends it, `didBecomeActive` resumes it. The observers run on the posting
/// thread (main), so `suspend()` may block main for up to 2 s: no GPU work may outlive the resign.
/// A failure is logged, never thrown, and `resume()` is still called on activation, because a
/// timed-out suspend leaves the session suspended.
// Nonisolated: the observer closures are @Sendable, so they capture only engine and logger.
nonisolated final class EngineLifecycleObserver<Engine: EngineDriving> {

    private let engine             : Engine
    private let notificationCenter : NotificationCenter
    private let logger             = Logger(subsystem: "com.eliorodr2104.unveil", category: "EngineLifecycle")

    // The notification token type is an existential declared by Foundation.
    private var tokens: [any NSObjectProtocol] = []

    init(engine: Engine, notificationCenter: NotificationCenter = .default) {
        self.engine             = engine
        self.notificationCenter = notificationCenter
    }

    deinit {
        for token in tokens {
            notificationCenter.removeObserver(token)
        }
    }

    /// Starts listening: resign active suspends the engine, become active resumes it.
    func start() {
        guard tokens.isEmpty else { return }

        let engine = engine
        let logger = logger

        // queue nil: runs on the posting thread, outside MainActor isolation.
        tokens.append(notificationCenter.addObserver(
            forName : UIApplication.willResignActiveNotification,
            object  : nil,
            queue   : nil
        ) { _ in
            do    { try engine.suspend() }
            catch { logger.error("Engine suspend failed: \(String(describing: error), privacy: .public)") }
        })

        tokens.append(notificationCenter.addObserver(
            forName : UIApplication.didBecomeActiveNotification,
            object  : nil,
            queue   : nil
        ) { _ in
            do    { try engine.resume() }
            catch { logger.error("Engine resume failed: \(String(describing: error), privacy: .public)") }
        })
    }
}
