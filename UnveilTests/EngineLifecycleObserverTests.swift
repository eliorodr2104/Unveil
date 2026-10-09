//
//  EngineLifecycleObserverTests.swift
//  UnveilTests
//

import Testing
import UIKit
@testable import Unveil

struct EngineLifecycleObserverTests {

    @Test
    func resigningActiveSuspendsAndBecomingActiveResumes() {
        let center   = NotificationCenter()
        let engine   = FakeEngine()
        let observer = EngineLifecycleObserver(engine: engine, notificationCenter: center)
        observer.start()

        center.post(name: UIApplication.willResignActiveNotification, object: nil)
        center.post(name: UIApplication.didBecomeActiveNotification, object: nil)

        #expect(engine.lifecycle == ["suspend", "resume"])
    }
}
