//
//  MeasurementLaunchOptions.swift
//  Unveil
//

/// MeasurementLaunchOptions are the launch arguments that let an agent run T15's measurements with
/// no taps (`devicectl device process launch <bundle> -- <arguments>`):
///
/// - `-UnveilOpen <name>`: open `Documents/raw/<name>` through the editor once the app is active;
/// - `-UnveilDelay <s>`: wait that long first (default 0), so xctrace can attach before the open;
/// - `-UnveilSweep <s>`: after the first full frame, run StressSweep for that many seconds;
/// - `-UnveilEngineDir fresh`: open the engine on a new, empty library, so the open is cold;
/// - `-UnveilExitAfterSweep`: end the process when the sweep ends (0 if it ran its full time).
///
/// They are parsed by hand, not through UserDefaults, because the last one is a flag with no value.
/// A number that does not parse, or is negative, or not finite, counts as absent.
nonisolated struct MeasurementLaunchOptions: Equatable, Sendable {

    let openFileName          : String?
    let delaySeconds          : Double
    let sweepSeconds          : Double?
    let isFreshEngineDirectory: Bool
    let exitsAfterSweep       : Bool

    init(arguments: [String]) {
        func value(after flag: String) -> String? {
            guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
            return arguments[index + 1]
        }

        func seconds(after flag: String) -> Double? {
            guard let number = value(after: flag).flatMap(Double.init),
                  number.isFinite,
                  number >= 0
            else {
                return nil
            }

            return number
        }

        openFileName           = value(after: "-UnveilOpen")
        delaySeconds           = seconds(after: "-UnveilDelay") ?? 0
        sweepSeconds           = seconds(after: "-UnveilSweep").flatMap { $0 > 0 ? $0 : nil }
        isFreshEngineDirectory = value(after: "-UnveilEngineDir") == "fresh"
        exitsAfterSweep        = arguments.contains("-UnveilExitAfterSweep")
    }
}
