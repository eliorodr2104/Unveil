//
//  EngineError.swift
//  Unveil
//

import UnveilEngine

// nonisolated: the engine thread and the canvas use this off the main actor, and the app default is MainActor.
/// EngineError is every failure the engine reports to the app, one case per UVStatus code.
/// The message is the engine's own text, meant for logs and the editor's error alert.
nonisolated enum EngineError: Error, Equatable {

    case invalidArgument(String)
    case unknownCommand(String)
    case engine(String)
    case panic(String)
    case suspended(String)
    case io(String)
    case timeout(String)
    case unknown(code: Int32, message: String)

    /// init(status:message:) maps a raw UVStatus code to its case. A code this build does not know
    /// becomes .unknown instead of trapping, so a newer engine cannot crash an older app.
    init(status: Int32, message: String) {
        switch status {
            case UV_ERR_INVALID_ARGUMENT.rawValue: self = .invalidArgument(message)
            case UV_ERR_UNKNOWN_COMMAND.rawValue:  self = .unknownCommand(message)
            case UV_ERR_ENGINE.rawValue:           self = .engine(message)
            case UV_ERR_PANIC.rawValue:            self = .panic(message)
            case UV_ERR_SUSPENDED.rawValue:        self = .suspended(message)
            case UV_ERR_IO.rawValue:               self = .io(message)
            case UV_ERR_TIMEOUT.rawValue:          self = .timeout(message)
            default:                               self = .unknown(code: status, message: message)
        }
    }
}
