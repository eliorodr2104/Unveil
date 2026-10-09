//
//  AdjustmentSlider.swift
//  Unveil
//

import SwiftUI

/// AdjustmentSlider is one develop control: its name, its current value and a slider over the
/// kind's range and step.
///
/// It knows nothing about the engine. The value comes in as a binding and the gesture edges go out
/// through `onEditingChanged`, so the panel decides what a drag means (begin, drag, end).
///
/// For VoiceOver the row is one element: the name and the value are read once, and a swipe up or
/// down moves the value by one step through `onAccessibilityAdjust`. A VoiceOver adjustment never
/// calls `onEditingChanged`, so it gets its own callback: the panel turns it into a complete edit.
struct AdjustmentSlider: View {

    let kind                 : DevelopAdjustmentKind
    let value                : Binding<Double>
    let onEditingChanged     : (Bool) -> Void
    let onAccessibilityAdjust: (Double) -> Void

    @Environment(\.isEnabled)
    private var isEnabled

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {

            HStack {

                Text(title)

                Spacer()

                Text(formattedValue)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .font(.subheadline)

            Slider(
                value            : value,
                in               : kind.range,
                step             : kind.step,
                onEditingChanged : onEditingChanged
            )
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(formattedValue)
        .accessibilityAdjustableAction { direction in adjust(direction) }
    }

    /// adjust moves the value one step in the VoiceOver direction, clamped to the range. A disabled
    /// slider (no photo open) ignores it, as its touch gesture does.
    private func adjust(_ direction: AccessibilityAdjustmentDirection) {
        guard isEnabled else { return }

        let delta: Double

        switch direction {
            case .increment:
                delta = kind.step

            case .decrement:
                delta = -kind.step

            @unknown default:
                return
        }

        let stepped = value.wrappedValue + delta
        onAccessibilityAdjust(min(max(stepped, kind.range.lowerBound), kind.range.upperBound))
    }

    /// title is the control's name as the panel shows it.
    private var title: String {
        switch kind {
            case .exposure:    "Exposure"
            case .contrast:    "Contrast"
            case .highlights:  "Highlights"
            case .shadows:     "Shadows"
            case .whites:      "Whites"
            case .blacks:      "Blacks"
            case .temperature: "Temperature"
            case .tint:        "Tint"
            case .vibrance:    "Vibrance"
            case .saturation:  "Saturation"
        }
    }

    /// formattedValue shows as many decimals as the step has: two for exposure, none for the rest.
    private var formattedValue: String {
        let decimals = kind.step < 1 ? 2 : 0
        let text     = value.wrappedValue.formatted(.number.precision(.fractionLength(decimals)))

        return kind == .temperature ? "\(text) K" : text
    }
}
