//
//  EngineLinkTests.swift
//  UnveilTests
//

import Testing
import UnveilEngine

struct EngineLinkTests {

    @Test
    func abiVersionMatchesHeader() {
        #expect(uv_abi_version() == UInt32(UV_ABI_VERSION))
    }
}
