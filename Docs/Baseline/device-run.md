# Device run: iPad Air 11" (M2)

Date: 2026-10-09. Device: iPad Air 11-inch (M2), iPad14,8, iPadOS 27. App: Unveil Release builds from the
`elio/v0-baseline` line, signed with a personal team (Increased Memory Limit only; Extended Virtual
Addressing is not available to personal teams). Engine backend at launch: `Apple M2 GPU (Metal)`, available.

## Install and launch

| Check | Result | Evidence |
|---|---|---|
| Signed install and launch | OK | `xcrun devicectl device install app` and `process launch` after the user trusted the developer profile |
| App size | 33 MB | Release `.app` with the engine linked |
| Unit and integration tests on the device | OK | 41/41 in 11 suites (`xcodebuild test` on the iPad) |

## Hands-on checks (by the author)

| # | Check | Result |
|---|---|---|
| 1 | JPEG: open, ten sliders, draft while dragging, sharp on release | OK, "very fluid" |
| 2 | RAW ~26 MP (Fujifilm X-T4, X-Trans RAF): open and edit | OK; opening feels slow (measured in T15) |
| 3 | RAW ~46 MP (Nikon Z 7 NEF): open and edit | OK; opening feels slow (measured in T15) |
| 4 | Canvas: pinch, pan, double tap, rotation | OK |
| 5 | All ten sliders | OK |
| 6 | Leave the app in the middle of a drag, come back: no alert, image returns sharp, Engine state shows `reason` and `lastFallback` empty | OK |
| 7 | Same with Control Center pulled down mid-drag | OK |
| 8 | Open `not-a-photo.ARW`: readable error, app stays open | OK |
| 9 | Torn frames seen | None |

## Found on the device

- iOS moves the app's data container on reinstall. A photo opened before a reinstall kept a path into the old
  container, and reopening it never delivered a frame. Fixed in `07fee5d` (relink on duplicate or restored).
- A render whose source cannot be loaded produces neither a frame nor an error across the C ABI. Open item,
  outside v0: add an error path to the frame callback.
