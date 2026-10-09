//
//  AdjustmentPanel.swift
//  Unveil
//

import SwiftUI

/// AdjustmentPanel lists the ten develop sliders and turns each slider gesture into the view
/// model's begin, drag and end calls.
///
/// The sliders stay disabled until a photo is open: there is nothing for the engine to apply them to.
/// Drags start with `Task.immediate`, so each call runs its synchronous part (clamp, record, queue)
/// right away, in the order SwiftUI delivered the values. A plain `Task` would only be scheduled,
/// and the release could then queue its full preview ahead of the last drag value.
struct AdjustmentPanel<Engine: EngineDriving, Importer: PhotoImporting>: View {

    let viewModel: EditorViewModel<Engine, Importer>

    var body: some View {
        ScrollView {

            VStack(spacing: 20) {

                ForEach(DevelopAdjustmentKind.allCases, id: \.self) { kind in
                    AdjustmentSlider(
                        kind                  : kind,
                        value                 : binding(for: kind),
                        onEditingChanged      : { editingChanged(kind, isEditing: $0) },
                        onAccessibilityAdjust : { accessibilityAdjust(kind, to: $0) }
                    )
                }
            }
            .padding()
        }
        .disabled(!viewModel.isPhotoOpen)
    }

    /// binding reads the model's value for `kind` and sends every new slider value as a drag.
    private func binding(for kind: DevelopAdjustmentKind) -> Binding<Double> {
        Binding(
            get: { viewModel.values[kind] ?? kind.defaultValue },
            set: { newValue in Task.immediate { await viewModel.drag(kind, to: newValue) } }
        )
    }

    /// accessibilityAdjust runs one VoiceOver step as a whole edit: set the value, then a full
    /// preview. Each step is final, so it must not stop on a draft frame.
    private func accessibilityAdjust(
        _ kind     : DevelopAdjustmentKind,
        to newValue: Double
    ) {
        Task.immediate {
            viewModel.beginDrag(kind)
            await viewModel.drag(kind, to: newValue)
            await viewModel.endDrag(kind)
        }
    }

    private func editingChanged(
        _ kind   : DevelopAdjustmentKind,
        isEditing: Bool
    ) {
        if isEditing {
            viewModel.beginDrag(kind)
        } else {
            Task.immediate { await viewModel.endDrag(kind) }
        }
    }
}
