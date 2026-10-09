# CODE_STYLE.md

Code style conventions for **Unveil**. Goal: **protocol-oriented, explicit, type-safe** code that reads well, with a clear seam between the *style* rules (which never bend) and the *performance* rules (which let us reach for the unsafe, low-level tools where the pixel path demands it).

Unveil is an **iPad RAW editor**: **UIKit** for the app shell, the editor and its canvas, **SwiftUI** for panels and leaf components only, and a **Rust engine** in `Engine/` (the vendored LightCraft workspace plus the `Engine/ffi` bridge) that does the decoding and the pixel work. The deployment floor is **iPadOS 26**. The engine bridge is designed as if it were a library: clean contracts, no leaking internals.

## Principles

1. **Protocol-oriented first, and harder than usual.** Depend on abstractions (protocols), never on concrete implementations. Every service / resolver / monitor / engine / renderer has its protocol so callers know only the contract and the app stays testable and mockable. This is the backbone of the whole project: the editor depends on an engine protocol, not on the FFI bridge directly.
2. **Immutable value models, by default.** Anything that describes state (adjustment values, image metadata, a render request) is a value `struct` with `let` properties. Mutable, framework-coupled state belongs to the UI layer (`@Observable`), not to the model.
3. **Performance is a first-class constraint.** Unveil must not devour RAM, CPU or battery, and must never hang the UI. In the hot pixel / render path we deliberately use `@frozen`, compact and aligned structs, contiguous storage, `InlineArray` and `Unsafe*` pointers for cache-friendly, allocation-free access. This is the **one** place where we trade safety for speed, explicitly, behind a clean API, never leaking the unsafety to callers. See **Performance-critical code**.
4. **No unsafe unwraps in ordinary code.** `!` (force unwrap) and `try!` are forbidden outside the audited performance path. Use `guard let` / `if let`, `??`, or explicit error handling.
5. **Explicit over clever.** Readability comes before one-line tricks, even in the fast path: comment the trick.
6. **Resources are part of the contract.** A subsystem that wakes the CPU on a timer, blocks the main thread or re-renders without a real input change is a *bug*, not a style nit. The type system and the protocols are shaped to make the cheap path the default one. See **Performance-critical code**.

## File header

Every file opens with the standard Xcode banner: the file name and the product name. The product is `Unveil` for the app target and `UnveilTests` for the test target. The `Created by` line is optional: keep it if it is there, drop it if it is not; don't churn it.

```swift
//
//  CanvasLayer.swift
//  Unveil
//
```

## Naming

- **Never use cryptic names. The more explanatory, the better.** A slightly longer name that says what it is always beats a short one that needs a comment.
- Prefer `activeImage` over `img`, `canvasScale` over `cs`, `redrawProgress` over `rp`. No abbreviations unless they are universal (`url`, `id`, `dpi`, `rgb`).
- Types: `UpperCamelCase`. Members: `lowerCamelCase`.
- Protocols: role name or capability suffix (`EngineDriving`, `PhotoImporting`, `AvailableMemoryReading`).
- Concrete implementations: a qualifier that states their nature (`EngineManager`, `PhotoImporter`, `MemoryBudgetMonitor`).
- Booleans read as questions (`isOpen`, `hasDecodedImage`, `canZoomIn`, `isSuspended`).

## Comments: Antirez style, in English

Comments are written **in English**, in the style of Salvatore Sanfilippo (antirez): generous, narrative, and focused on the **why**, not the what. A comment that just restates the code is noise; a comment that explains intent, trade-offs and gotchas is gold. This matters doubly in the pixel / rendering code, where the *why* (a tone-curve control-point layout, a color-space conversion, a coordinate flip, a memory-layout choice) is rarely obvious from the code alone.

- **Doc comments use `///`, never `/* … */`.** Triple-slash is the standard for every type, function and property worth documenting.
- Put a `///` doc comment above every non-trivial type and function describing what it does, why it exists, and any non-obvious behavior or edge case.
- **Open the doc comment with the symbol's own name as the subject** ("CanvasTransform describes…", "EngineManager drives…") so it reads as a definition.
- Use full English sentences. Explain reasoning, assumptions, and the things that would surprise the next reader.
- In the performance path, **always document the unsafe contract**: who owns the buffer, what the pointer's lifetime is, why the bounds are safe.
- Keep comments in sync with the code. Never leave commented-out code in the repo.
- **No em-dashes in comments or prose.** Use a colon, a comma or parentheses.

```swift
/// CanvasTransform maps the image into the view: aspect fit at scale 1, then zoom and pan.
///
/// It is a plain value, a pure function of the gesture state, so this struct
/// carries only the numbers the Metal layer turns into the image quad. It is
/// `@frozen` because its layout is stable and the redraw path reads it on every
/// frame; freezing lets the compiler lay it out at compile time and keeps the
/// read cache-friendly.
@frozen
struct CanvasTransform: Equatable, Sendable {

    var zoom  : Double = 1
    var offset: SIMD2<Double> = .zero

    // quad(viewSize:imageSize:), pinch, pan and reset are elided here.
}
```

## Alignment: line things up in columns

**Align in columns wherever possible.** Vertical alignment makes related lines scan as a table and surfaces inconsistencies immediately.

- The colon stays attached to the name (Swift convention), but pad **before** the colon to align the types.
- Align `=` in groups of related assignments.
- Align enum raw values and `OptionSet` members.
- Align trailing comments.
- **This applies to call sites too, not only declarations:** align the argument labels of a multi-line call or initializer.
- **Multi-argument calls and declarations break across lines, paren-on-its-own.** A declaration with more than one parameter, and a call with three or more arguments or one that would run past 100 columns, puts the **opening parenthesis at the end of its line**, every argument on **its own indented line** (labels aligned in columns), and the **closing parenthesis alone** on the final line. Never crowd the first argument onto the call line and hang the rest off it. A call with one or two short arguments, an unlabeled C call (`vDSP_…`, `CF…`), a short SwiftUI modifier and a loop header such as `stride(from:to:by:)` stay on one line.

```swift
enum DevelopAdjustmentKind: String, CaseIterable, Sendable {
    case exposure    = "light.exposure"
    case contrast    = "light.contrast"
    case temperature = "wb.temp"
    case vibrance    = "color.vibrance"
    // ...
}

struct ReadyFrame: Equatable, Sendable {
    let bufferIndex : Int      // Which of the two shared MTLBuffers holds the pixels.
    let width       : Int
    let height      : Int
    let bytesPerRow : Int
    let generation  : UInt64   // FrameSink drops anything at or below the last published one.
    let isDraft     : Bool
}

let minimumZoom   = 1.0
let maximumZoom   = 16.0
let previewPixels = 2048

// Call sites align their labels the same way: open paren alone, one argument per
// line, close paren alone:
let viewModel = EditorViewModel(
    engine        : engine,
    importer      : importer,
    previewPixels : previewPixels
)

// Not this: first argument crowded onto the call line, the rest hanging off it:
let viewModel = EditorViewModel(engine        : engine,
                                importer      : importer,
                                previewPixels : previewPixels)
```

When alignment would fight the compiler or hurt readability (very long lines, generics), readability wins, but reach for columns by default.

## Models

- Value `struct`, immutable (`let`). `Equatable` (and `Hashable` / `Identifiable` where it makes sense) is the norm.
- `ReadyFrame` and `PhotoID` are the shape: `let` value structs, `Equatable` or `Hashable`, `Sendable`, so they cross from the engine thread to the canvas without sharing mutable state.
- **Framework-agnostic at the data level**: a model that only describes data imports nothing from SwiftUI or Combine. A model does not hold a `Color`: it carries four sRGB `Double` components, each from 0 to 1, or a packed ARGB `UInt32` (`0xAARRGGBB`) if it must persist.
- No presentation state (hover, focus, animation progress) in pure data models: that belongs to the UI / state layer.
- The tone-curve control-point buffers are **not** `Codable` models: they are performance-critical storage (see **Performance-critical code**). Keep the two concepts apart: descriptive models are immutable value types; pixel and curve buffers are compact, contiguous storage.

## Services / Resolvers / Monitors / Engines / Renderers (protocol-oriented)

```swift
protocol EngineDriving: AnyObject, Sendable {

    var frames: FrameSink { get }

    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID
    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64
    // set(_:to:), suspend(), resume() and setMemoryBudget(bytes:) are elided here.
}

/// EngineManager is the only door to the engine: no other type calls `uv_*`.
/// It owns the session handle and the FrameSink its callback fills.
final class EngineManager: EngineDriving {

    init(maxPixels: Int) throws(EngineError)
    // The rest of the body is elided here.
}

/// FrameSink holds two shared MTLBuffers that alternate (double buffering).
/// The engine thread calls `receive(...)`; the canvas reads `latest()`.
final class FrameSink: Sendable {

    init(device: some MTLDevice, maxPixels: Int)
    func latest() -> ReadyFrame?
    // receive(rgba:width:height:stride:generation:isDraft:) is elided here.
}

/// CanvasView draws the newest ready frame through its CAMetalLayer. There is no
/// polling: a CADisplayLink is armed only when a new frame is pending (redraw on
/// demand) and paused once it is drawn. No per-frame main-thread hop: one
/// coalesced hop wakes the canvas, and the pixels are read from the sink.
final class CanvasView: UIView {

    init(frames: FrameSink, device: some MTLDevice)
    func setNeedsRedraw()
}
```

- The protocol is the contract; callers only know the protocol. `FrameSink` is the deliberate exception: a concrete `final class` on the pixel path, so no existential and no dynamic dispatch per frame.
- Dependencies are injected through `init`.
- Anything that touches UIKit windows, Core Animation, system events or screen geometry sits behind a protocol so it can be stubbed in tests without a real display or touch stream.

## State holders (UI-facing)

This app targets **iPadOS 26**, so it uses the modern **Observation** framework, not the legacy `ObservableObject`.

- **Use `@Observable`** for state holders the SwiftUI layer reads. Its fine-grained tracking means a view re-renders only when a property it actually reads changes, which is exactly the re-render discipline the project demands.
- State holders that own UI-visible domain state are `final class`, `@MainActor`.
- State that a background context produces (decoded image, loaded data) must be published to the UI **on the main actor**; never let a view observe a value being written from a background queue.
- **Property wrappers go on their own line**, above the declaration, never inline:

```swift
@Observable
@MainActor
final class EditorViewModel<Engine: EngineDriving, Importer: PhotoImporting> {

    private(set) var values      : [DevelopAdjustmentKind: Double]
    private(set) var openPhoto   : PhotoID?
    private(set) var errorMessage: String?

    // init, open(pickedURL:) and the drag methods are elided here.
}

// The UIKit editor owns its model as a plain `let`: no wrapper, no re-wrapping.
final class EditorViewController<Engine: EngineDriving, Importer: PhotoImporting>: UIViewController {

    private let viewModel: EditorViewModel<Engine, Importer>
}
```

## Extensions

- **Only create an extension when it earns its place.** If the code can live directly in the type's own file, put it there. A private helper used only inside `Foo` belongs in `Foo.swift`, not in a separate extension.
- When an extension is justified (cross-cutting additions to types you don't own, such as Foundation, UIKit or Core Graphics, or helpers shared by several files), give it its own file.
- One file per extended type and capability, named `Type+Capability.swift`. The `+` makes it obvious what the file adds: `CGAffineTransform+Canvas.swift`, `UIScreen+RefreshRate.swift`.
- Do not pile unrelated helpers into one giant `UIScreen+Extensions.swift`: split by capability.

## Concurrency: separated contexts

The app's smoothness depends on keeping three classes of work apart. Mixing them causes hangs, dropped frames or battery drain.

1. **Render server (system-owned, real-time).** We feed it layer changes inside a `CATransaction`. **Never** stall the commit with disk reads, allocation, locks or heavy compute: anything that blocks shows up as a dropped frame.
2. **Main thread / main actor (interactive, 120 Hz).** UIKit, SwiftUI, gesture handling, the `CADisplayLink` callback. Handles user input and layout only. Use `@MainActor` for anything UI-visible. A hang here freezes the editor.
3. **Background worker (utility / background QoS).** `Task.detached` or a dedicated `DispatchQueue`: data loading, image decode, geometry / persistence caches.

Rules:

- `async`/`await` for application asynchronous work. Keep platform-required callbacks, such as Objective-C completion blocks, inside adapters and expose async interfaces to consumers.
- Heavy work (decode, IO, parse) stays off the main actor and never stalls the compositor.
- Cross the actor boundary explicitly when handing a finished result back to the UI; do not let observation reach into background-mutated state.

## Performance-critical code (the fast path)

This is the part that justifies the careful style. Treat it as a small, audited blast radius where the normal "no unsafe" rule is relaxed *on purpose* and *visibly*.

- **`@frozen` on hot structs** the compiler benefits from laying out at compile time (`CanvasTransform`, control points). It enables cache hits and a stable layout; only freeze what is genuinely stable across the app's evolution.
- **Compact, aligned, contiguous storage.** Prefer parallel arrays / `ContiguousArray` / raw buffers over arrays-of-structs when the render loop or the renderer walks them with unit stride. Keep structs small and field order chosen for alignment.
- **`InlineArray` for fixed-size buffers.** A known, fixed number of elements (control points, a 3x3 matrix, a curve's knots) stays on the stack with no heap allocation and no ARC. It is Swift 6.2 stdlib and iPadOS 26 ships its runtime, so use it and `Span` / `MutableSpan` / `RawSpan` **without** availability checks.

- **`UnsafePointer` / `UnsafeMutablePointer`** for filling the curve lookup tables in the render loop, where bounds checks and ARC retain/release would dominate. Every use must:
  - be wrapped behind a safe API so callers never see a raw pointer,
  - document the buffer's owner and lifetime in a `///` comment,
  - guarantee the bounds before entering the unsafe region.
- **Do not allocate in the `CADisplayLink` callback**, and avoid per-frame allocation in the render loop generally. Pixel buffers are allocated once and reused.
- **Renderer:** draw into the `CAMetalLayer` canvas and redraw on demand from a `CADisplayLink` that is paused when idle, instead of overriding `draw(_:)`, so the GPU does the work. The canvas has its own layer; a pan or zoom must never trigger a recompute of unrelated layers. Target 120 Hz or better.
- When you optimize, **say what you traded and why** in a comment. An unexplained `UnsafeMutablePointer` is a future bug.

## Error handling

- Errors are modeled with dedicated, descriptive types (`enum … : Error`).
- `throws` for propagatable failures; `Result` only where the API requires it.
- Never swallow an error silently: handle it or propagate it. Log through a single logging seam (`os.Logger`).

## Folder layout

Each target is laid out by layer, not by feature. Inside a layer, an area gets its own folder once it has more than a couple of files.

```text
Unveil/
├─ Unveil.xcodeproj
├─ Unveil/
│  ├─ App/           AppDelegate, SceneDelegate, service composition
│  ├─ Core/          Engine/ Memory/ Lifecycle/ Import/ Diagnostics/
│  │  ├─ Protocols/  one protocol per file
│  │  ├─ Errors/     one Error type per file
│  │  └─ Extensions/ Type+Capability.swift
│  ├─ Models/        by area; enums in Models/<Area>/Enums/
│  ├─ Views/         Editor/ Diagnostics/
│  ├─ Components/    Adjustments/
│  └─ Resources/
├─ UnveilTests/
├─ Engine/           Rust workspace: crates/ (upstream engine included), assets/, ffi/ (include/, src/, tests/), UPSTREAM.md
├─ Frameworks/       generated XCFramework, ignored by git
├─ Config/           UnveilEngine.xcconfig (generated link flags)
├─ scripts/          build-xcframework.sh, test-app.sh
├─ Docs/             Specs/ Plans/ Baseline/
└─ CODE_STYLE.md  CLAUDE.md  LICENSE  NOTICE  README.md
```

## Types per file

- **One type per file**, named after it. A second type is allowed only when it is a small private helper of the first (a representable, a cell, a keyframe value); never more than two.
- Nested types do not count, and neither do extensions of the file's own type (conformances stay in its file). An extension of another type goes to `Core/Extensions/Type+Capability.swift`.
- Protocols, errors, value models and their enums live in their own layers (see **Folder layout**), not next to the implementation that uses them. A type that moves away from its only user loses `private`.
- The exception is a type that only one other type may create or read: a token, ticket or capability whose initializer or fields are `fileprivate` so that its minter alone can make it. It stays in its minter's file, however many of them there are, because moving it out would mean widening that access. Working types with short names that serve a single type (`State`, `Outcome`) are nested inside it instead of living at the top of the file.

## Vertical rhythm

The code should breathe (this vertical rhythm comes from the author's earlier ReixOS style; Unveil does not depend on it):

- **One blank line after the opening brace of every type, extension and protocol**, none before its closing brace, and one between members. Stored properties that belong together form one aligned group without blank lines; separate groups (constants, dependencies, state) with one.
- Inside a function, **separate logical steps with a blank line**: the guards, the work, the result. No blank line directly after a function's opening brace; a body of one step has none at all.
- **Attributes and property wrappers on their own line** above the declaration: `@Environment(…)`, `@State`, `@ObservationIgnored`, `@objc`, `@discardableResult`, `@available`.
- **A guard that spans lines** puts each condition on its own line, aligned after `guard `, and `else` on a line of its own. A guard that fits on one line stays on one line.
- **`case` is indented one level inside `switch`**, and cases whose bodies run longer than a line are separated by a blank line.
- One enum case per line when cases carry raw or associated values; their values align.

```swift
final class CanvasView: UIView {

    private var displayLink: CADisplayLink?
    private var isDirty    : Bool = false

    private func tearDownDisplayLink() {
        displayLink?.invalidate()
        displayLink = nil

        isDirty = false
    }
}
```

## File organization and access control

- One type per file; file name = type name (see **Types per file**). The file opens with the header banner (see **File header**).
- `private` / `fileprivate` for everything that is not part of the public contract.
- `private(set)` for read-only exposed state.
- Use extensions to separate protocol conformances (`extension Foo: SomeProtocol { … }`), kept in the type's own file unless the Extensions rule above applies.
- `// MARK: -` to separate sections of a long file.

## Formatting

- 4-space indentation.
- Lines roughly <= 100–120 characters.
- One declaration per line; spaces around operators.
- Property wrappers on their own line (see **State holders**).
- No commented-out code left in the repo.
- **One modifier per line.** Don't chain view / builder modifiers on a single line: give each `.modifier(…)` its own line. `Image(...).font(...).foregroundStyle(...)` becomes three lines.
- **Favor vertical breathing room over dense packing.** Separate sibling views and distinct logical groups with a blank line, and break a call's arguments onto their own lines (see **Alignment**) instead of packing them across. When in doubt, give it *more* room. The code should breathe, not cram.

```swift
VStack(spacing: 8) {

    Image(systemName: "photo")
        .font(.headline)
        .foregroundStyle(.secondary)

    Text("No photo open.")
        .foregroundStyle(.secondary)
}
.frame(maxWidth: .infinity, maxHeight: .infinity)
```

## Unveil additions

- **UIKit, not AppKit.** SwiftUI only for panels and leaf components. On iPadOS 26, `InlineArray` and `Span` need no version checks.
- **`//` comments are at most 2 lines.** If more is needed, it goes in a `///` on the symbol. `///` stays narrative, in English, antirez style.
- **Static protocol orientation.** Use constrained generics and `some`, not `any` existentials. `any` only where a heterogeneous type is truly needed, with a comment that justifies it. In Rust: generics and `impl Trait`, no `dyn` on the hot path.
- **Stack first, then heap.** Values, `InlineArray` and `Span` / `MutableSpan` / `RawSpan` reach buffers without copies or ARC. The heap is used only for a measured gain or when unavoidable. Pixel buffers live in memory allocated once and reused, never allocated per frame.
- **On the hot path, low level wins.** `@frozen`, compact aligned layouts, `Unsafe*` behind safe APIs with the contract documented. Every such choice is justified by a measurement.
- **Concurrency:** shared state lives behind a `Mutex` (Synchronization), no actors. `DispatchSemaphore` only as a signal between dedicated threads, always with a timeout, never on the main thread.
- **Rust:**
  - `rustfmt` with 4 spaces and `max_width = 110` applies to `Engine/ffi` only (its own `rustfmt.toml`), with no manual alignment.
  - The vendored upstream code under `Engine/crates` keeps upstream's `rustfmt.toml` (`max_width = 150`) and is not reformatted in v0.
  - Files in snake_case, one module per responsibility.
  - Every `unsafe` block has a `// SAFETY:` comment (at most 2 lines) saying who owns the buffer and for how long.
