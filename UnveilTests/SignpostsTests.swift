//
//  SignpostsTests.swift
//  UnveilTests
//

import Foundation
import Testing
@testable import Unveil

/// SignpostsTests covers the pure parts of the measurement code: the frame matching behind the
/// signposts, the launch arguments, the CSV row and the sweep's schedule.
struct SignpostsTests {

    @Test
    func aPresentedGenerationEndsEveryIntervalAtOrBelowIt() {
        var pending = PendingFrameIntervals<String>()

        let added = [
            pending.add("draft 3", waitingFor: 3),
            pending.add("draft 4", waitingFor: 4),
            pending.add("full 5", waitingFor: 5),
            pending.add("full 7", waitingFor: 7),
        ]
        let ended = pending.presented(5)

        #expect(added == [true, true, true, true])

        #expect(ended.map(\.token) == ["draft 3", "draft 4", "full 5"])
        #expect(ended.map(\.generation) == [3, 4, 5])
        #expect(pending.entries.map(\.token) == ["full 7"])
    }

    @Test
    func aLaterFrameEndsNoIntervalTwice() {
        var pending = PendingFrameIntervals<Int>()
        _ = pending.add(1, waitingFor: 1)
        _ = pending.add(2, waitingFor: 2)

        let first  = pending.presented(2)
        let second = pending.presented(2)
        let later  = pending.presented(9)

        #expect(first.map(\.token) == [1, 2])
        #expect(second.isEmpty)
        #expect(later.isEmpty)
        #expect(pending.entries.isEmpty)
    }

    @Test
    func anIntervalAddedAfterItsFrameWasPresentedIsRefused() {
        var pending = PendingFrameIntervals<Int>()
        _ = pending.presented(6)

        let atPresented = pending.add(1, waitingFor: 6)
        let afterIt     = pending.add(2, waitingFor: 7)

        #expect(atPresented == false)
        #expect(afterIt)
        #expect(pending.entries.map(\.token) == [2])
    }

    @Test
    func launchArgumentsParseEveryOption() {
        let options = MeasurementLaunchOptions(arguments: [
            "/app/Unveil",
            "-UnveilOpen", "DSCF0267.RAF",
            "-UnveilDelay", "8",
            "-UnveilSweep", "30",
            "-UnveilEngineDir", "fresh",
            "-UnveilExitAfterSweep",
        ])

        #expect(options.openFileName == "DSCF0267.RAF")
        #expect(options.delaySeconds == 8)
        #expect(options.sweepSeconds == 30)
        #expect(options.isFreshEngineDirectory)
        #expect(options.exitsAfterSweep)
    }

    @Test
    func aNormalLaunchHasNoMeasurementOptions() {
        let options = MeasurementLaunchOptions(arguments: ["/app/Unveil"])

        #expect(options.openFileName == nil)
        #expect(options.delaySeconds == 0)
        #expect(options.sweepSeconds == nil)
        #expect(options.isFreshEngineDirectory == false)
        #expect(options.exitsAfterSweep == false)
    }

    @Test
    func malformedNumbersAndAMissingValueCountAsAbsent() {
        let options = MeasurementLaunchOptions(arguments: [
            "-UnveilDelay", "-3",
            "-UnveilSweep", "inf",
            "-UnveilEngineDir", "old",
            "-UnveilOpen",
        ])

        #expect(options.openFileName == nil)
        #expect(options.delaySeconds == 0)
        #expect(options.sweepSeconds == nil)
        #expect(options.isFreshEngineDirectory == false)
    }

    @Test
    func aCSVRowHasFiveFieldsInHeaderOrder() {
        let row = FootprintSampler.row(
            time      : Date(timeIntervalSince1970: 0),
            elapsed   : 4.04,
            footprint : 123_456_789,
            available : 2_000_000_000,
            thermal   : .serious
        )

        #expect(row == "1970-01-01T00:00:00Z,4.0,123456789,2000000000,serious")
        #expect(FootprintSampler.header.split(separator: ",").count == 5)
        #expect(FootprintSampler.row(
            time      : Date(timeIntervalSince1970: 0),
            elapsed   : 0,
            footprint : nil,
            available : 0,
            thermal   : .nominal
        ) == "1970-01-01T00:00:00Z,0.0,,0,nominal")
    }

    @Test
    func thePhysicalFootprintIsReadable() {
        #expect((FootprintSampler.physicalFootprint() ?? 0) > 0)
    }

    @Test
    func theSweepSwingsBetweenMinusTwoAndTwoEveryFourSeconds() {
        typealias Sweep = StressSweep<FakeEngine, FakeImporter>

        #expect(abs(Sweep.exposure(at: 0)) < 1e-12)
        #expect(abs(Sweep.exposure(at: 1) - 2) < 1e-12)
        #expect(abs(Sweep.exposure(at: 3) + 2) < 1e-12)
        #expect(abs(Sweep.exposure(at: 4)) < 1e-12)

        let samples = stride(from: 0.0, to: 8, by: 1.0 / 60).map(Sweep.exposure(at:))
        #expect(samples.allSatisfy { abs($0) <= 2 })
    }

    @Test
    func theSweepReleasesOnceEverySecond() {
        typealias Sweep = StressSweep<FakeEngine, FakeImporter>

        let ticks    = stride(from: 0.0, through: 10, by: 1.0 / 60).map { $0 }
        let releases = zip(ticks, ticks.dropFirst()).filter { Sweep.isRelease(from: $0, to: $1) }

        #expect(releases.count == 10)
        #expect(Sweep.isRelease(from: 0.99, to: 1.0))
        #expect(Sweep.isRelease(from: 1.0, to: 1.01) == false)
    }
}
