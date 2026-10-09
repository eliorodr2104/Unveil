//
//  EngineDiagnosing.swift
//  Unveil
//

// Nonisolated: the diagnostics run on the engine's command queue, and the app default is MainActor.
/// EngineDiagnosing is the one door to the engine's diagnostic commands: `app.gpu`, `develop.reset`
/// and `library.memory`. The editor never needs them, so they stay out of EngineDriving and its fakes.
///
/// The result is the engine's JSON, untouched: the caller shows it or logs it, and decodes nothing.
nonisolated protocol EngineDiagnosing: AnyObject, Sendable {

    func diagnostic(_ command: String, params: some Encodable & Sendable) async throws(EngineError) -> String
}
