//
//  MeasurementLaunchOptions.swift
//  Unveil
//

import Foundation

/// MeasurementLaunchOptions are the launch arguments that let an agent run T15's measurements with
/// no taps (`devicectl device process launch <bundle> -- <arguments>`):
///
/// - `-UnveilOpen <name>`: open `Documents/raw/<name>` through the editor once the app is active;
/// - `-UnveilDelay <s>`: wait that long first (default 0), so xctrace can attach before the open;
/// - `-UnveilSweep <s>`: after the first full frame, run StressSweep for that many seconds;
/// - `-UnveilEngineDir fresh`: open the engine on a new, empty library, so the open is cold;
/// - `-UnveilExitAfterSweep`: end the process when the sweep ends (0 if it ran its full time);
/// - `-UnveilPanel <name>`: after the first full frame, open that adjustment panel ("Light",
///   "Color"...: the group title, any case), so a screenshot of a panel needs no tap;
/// - `-UnveilZoom <percent>`: after the first full frame, zoom to that percentage of the photo's real
///   pixels (clamped to the zoom range), so a screenshot of a zoom level needs no pinch;
/// - `-UnveilPanSweep`: during the sweep, also pan the canvas in a circle (CanvasView.measurementPan),
///   so a zoomed sweep moves the visible window as a finger would;
/// - `-UnveilZoomCycle`: instead of a sweep, zoom between fit and 100 % three times, then pinch to
///   400 % and back, while the memory CSV is written, printing how long each zoom takes to its first
///   window; a Debug build also writes the hitch table of each phase (FrameTimingProbe);
/// - `-UnveilExposure <ev>`: after the first full frame, set the exposure to that value (it may be
///   negative), committed like a released slider, so a screenshot of a clipping photo needs no drag;
/// - `-UnveilClipping`: after the first full frame, turn on both clipping overlays (they show with
///   the card, so pair it with `-UnveilPanel`);
/// - `-UnveilBefore`: after the first full frame (and the exposure), show the unedited look, so a
///   screenshot of the before view needs no press and hold;
/// - `-UnveilHomePhotos`: start the home's panel on Photos instead of Files;
/// - `-UnveilHomeExpanded`: start the home's panel at full height, so its screenshot needs no drag;
/// - `-UnveilRecordInput <s>`: a Debug build records the canvas's input events and display updates
///   for that many seconds (InputRecorder), from the first full frame of `-UnveilOpen` or else from
///   launch, so a hand-made trackpad session leaves a CSV; with `-UnveilExitAfterSweep` it then ends.
/// - `-UnveilMemoryMap`: print `MM` at open, fit +5/+15 s, 100 %, and return to fit +10/+30 s;
/// - `-UnveilDecodeMemory`: sample footprint every 100 ms and print bounded decode allocation events;
/// - `-UnveilPoolBytes <bytes>`: override the free GPU reserve for a paired diagnostic comparison;
/// - `-UnveilNoPoolIdle`: disable idle reserve trimming for that diagnostic process;
/// - `-UnveilPoolProbe`: measure an isolated free-pool trim and the first edit afterwards;
/// - `-UnveilMemoryCycles <1...3>`: repeat the memory map's zoom/fit sequence in one process;
/// - `-UnveilTimeline`: print the open's phases and the engine's file loads (`TL` lines);
/// - `-UnveilCost`: around the sweep, print its CPU, energy, GPU and render totals (`COST` line);
/// - `-UnveilSweepFull`: the sweep releases on every tick, so every frame is a full render.
///
/// `allowsImportSweep` is false under any of them, or any other `-Unveil...` argument (the
/// `-UnveilRun` disk report, say): see its doc.
///
/// They are parsed by hand, not through UserDefaults, because several are flags with no value.
/// A number that does not parse, or is negative, or not finite, counts as absent.
nonisolated struct MeasurementLaunchOptions: Equatable, Sendable {

    let openFileName          : String?
    let delaySeconds          : Double
    let sweepSeconds          : Double?
    let isFreshEngineDirectory: Bool
    let exitsAfterSweep       : Bool
    let openPanel             : AdjustmentGroup?
    let zoomPercent           : Double?
    let pansDuringSweep       : Bool
    let cyclesZoom            : Bool
    let exposure              : Double?
    let showsClipping         : Bool
    let showsBefore           : Bool
    let startsHomeOnPhotos    : Bool
    let startsHomeExpanded    : Bool
    let recordInputSeconds    : Double?
    let reportsMemoryMap      : Bool
    let reportsDecodeMemory   : Bool
    let poolLimitBytes        : UInt64?
    let disablesPoolIdle      : Bool
    let probesPool            : Bool
    let memoryCycles          : Int
    let reportsTimeline       : Bool
    let reportsCost           : Bool
    let releasesEveryTick     : Bool

    /// allowsImportSweep is the one gate of the import orphan sweep: true only for a plain launch,
    /// with no `-Unveil...` argument at all and not as the host of a test run (XCTest's
    /// `XCTestConfigurationFilePath` in the environment); the tests call the sweep directly.
    /// A fresh or other engine directory (`-UnveilEngineDir`) has a catalog that does not
    /// describe the shared Imports folder, so a sweep there would delete the real library's
    /// copies; a measurement or agent run must not change the disk it reads. Matching the
    /// prefix keeps a future option safe without a change here.
    let allowsImportSweep     : Bool

    init(
        arguments  : [String],
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        func value(after flag: String) -> String? {
            guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
            return arguments[index + 1]
        }

        func nonNegativeNumber(after flag: String) -> Double? {
            guard let number = value(after: flag).flatMap(Double.init),
                  number.isFinite,
                  number >= 0
            else {
                return nil
            }

            return number
        }

        func positiveNumber(after flag: String) -> Double? {
            nonNegativeNumber(after: flag).flatMap { $0 > 0 ? $0 : nil }
        }

        openFileName           = value(after: "-UnveilOpen")
        delaySeconds           = nonNegativeNumber(after: "-UnveilDelay") ?? 0
        sweepSeconds           = positiveNumber(after: "-UnveilSweep")
        isFreshEngineDirectory = value(after: "-UnveilEngineDir") == "fresh"
        exitsAfterSweep        = arguments.contains("-UnveilExitAfterSweep")
        openPanel              = value(after: "-UnveilPanel").flatMap { name in
            AdjustmentGroup.allCases.first { $0.title.lowercased() == name.lowercased() }
        }
        zoomPercent            = positiveNumber(after: "-UnveilZoom")
        pansDuringSweep        = arguments.contains("-UnveilPanSweep")
        cyclesZoom             = arguments.contains("-UnveilZoomCycle")
        exposure               = value(after: "-UnveilExposure").flatMap(Double.init).flatMap { $0.isFinite ? $0 : nil }
        showsClipping          = arguments.contains("-UnveilClipping")
        showsBefore            = arguments.contains("-UnveilBefore")
        startsHomeOnPhotos     = arguments.contains("-UnveilHomePhotos")
        startsHomeExpanded     = arguments.contains("-UnveilHomeExpanded")
        recordInputSeconds     = positiveNumber(after: "-UnveilRecordInput")
        reportsMemoryMap       = arguments.contains("-UnveilMemoryMap")
        reportsDecodeMemory    = arguments.contains("-UnveilDecodeMemory")
        poolLimitBytes         = value(after: "-UnveilPoolBytes").flatMap(UInt64.init)
        disablesPoolIdle       = arguments.contains("-UnveilNoPoolIdle")
        probesPool             = arguments.contains("-UnveilPoolProbe")
        memoryCycles           = value(after: "-UnveilMemoryCycles").flatMap(Int.init).map { min(3, max(1, $0)) } ?? 1
        reportsTimeline        = arguments.contains("-UnveilTimeline")
        reportsCost            = arguments.contains("-UnveilCost")
        releasesEveryTick      = arguments.contains("-UnveilSweepFull")
        allowsImportSweep      = !arguments.contains { $0.hasPrefix("-Unveil") }
            && environment["XCTestConfigurationFilePath"] == nil
    }
}
