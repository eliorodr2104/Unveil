//
//  AppDelegate.swift
//  Unveil
//

import UIKit
import UnveilEngine

/// AppDelegate is the process entry point; all UI setup lives in SceneDelegate.
@main
final class AppDelegate: UIResponder, UIApplicationDelegate {

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // The header and the linked library must agree on the C ABI. This `uv_*` reference
        // also keeps the symbol in the app for UnveilTests; EngineManager owns it from T9.
        precondition(uv_abi_version() == UInt32(UV_ABI_VERSION), "engine ABI mismatch")

        return true
    }
}
