//
//  EngineErrorTests.swift
//  UnveilTests
//

import Testing
import UnveilEngine
@testable import Unveil

struct EngineErrorTests {

    @Test
    func everyStatusMapsToItsCase() {
        #expect(EngineError(status: UV_ERR_INVALID_ARGUMENT.rawValue, message: "m") == .invalidArgument("m"))
        #expect(EngineError(status: UV_ERR_UNKNOWN_COMMAND.rawValue, message: "m") == .unknownCommand("m"))
        #expect(EngineError(status: UV_ERR_ENGINE.rawValue, message: "m") == .engine("m"))
        #expect(EngineError(status: UV_ERR_PANIC.rawValue, message: "m") == .panic("m"))
        #expect(EngineError(status: UV_ERR_SUSPENDED.rawValue, message: "m") == .suspended("m"))
        #expect(EngineError(status: UV_ERR_IO.rawValue, message: "m") == .io("m"))
        #expect(EngineError(status: UV_ERR_TIMEOUT.rawValue, message: "m") == .timeout("m"))
    }

    @Test
    func anUnknownCodeNeverCrashes() {
        #expect(EngineError(status: -99, message: "m") == .unknown(code: -99, message: "m"))
    }
}
