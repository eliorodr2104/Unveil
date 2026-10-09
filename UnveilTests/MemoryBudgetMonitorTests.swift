//
//  MemoryBudgetMonitorTests.swift
//  UnveilTests
//

import Testing
@testable import Unveil

struct MemoryBudgetMonitorTests {

    @Test
    func budgetIsAThirdOfAvailableMemory() {
        typealias Monitor = MemoryBudgetMonitor<FakeEngine, FixedMemory>
        #expect(Monitor.budget(forAvailable: 3 << 30) == 1 << 30)
    }

    @Test
    func budgetNeverFallsBelowTheFloor() {
        typealias Monitor = MemoryBudgetMonitor<FakeEngine, FixedMemory>
        #expect(Monitor.budget(forAvailable: 100 << 20) == 256 << 20)
    }

    @Test
    func aMemoryWarningHalvesTheBudget() {
        let engine  = FakeEngine()
        let monitor = MemoryBudgetMonitor(engine: engine, memory: FixedMemory(bytes: 3 << 30))
        monitor.start()
        monitor.handleMemoryWarning()
        #expect(engine.budgets == [1 << 30, 512 << 20])
    }

    @Test
    func aWarningKeepsTheFloor() {
        let engine  = FakeEngine()
        let monitor = MemoryBudgetMonitor(engine: engine, memory: FixedMemory(bytes: 1_288_490_188))
        monitor.start()
        monitor.handleMemoryWarning()
        #expect(engine.budgets == [429_496_729, 256 << 20])
    }

    @Test
    func aZeroReadingAppliesNoBudget() {
        let engine  = FakeEngine()
        let monitor = MemoryBudgetMonitor(engine: engine, memory: FixedMemory(bytes: 0))
        monitor.start()
        monitor.handleMemoryWarning()
        #expect(engine.budgets == [])
    }
}
