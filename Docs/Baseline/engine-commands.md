# Engine commands for v0 (T4)

The sequence Swift runs through `uv_execute(session, command, params_json, &result)`. Results below are real output
from `Engine/ffi/tests/commands.rs` (library in a temp dir, 96x64 PNG fixture).

## Status mapping

| Case | Status | `uv_last_error` |
|---|---|---|
| `params_json` is not valid JSON | `UV_ERR_INVALID_ARGUMENT` | `params_json: ...` (serde_json message) |
| command name unknown | `UV_ERR_UNKNOWN_COMMAND` | from the engine |
| any other engine failure | `UV_ERR_ENGINE` | engine message, e.g. `invalid parameters for `develop.set`: unknown control `nope.x`` |
| `develop.set` with no active photo | `UV_ERR_ENGINE` | contains `no photo selected` |

A bad control id is an engine error (`UV_ERR_ENGINE`), not `UV_ERR_INVALID_ARGUMENT`: only the JSON syntax is checked in the FFI.
`params_json` NULL means `{}`. `develop.set` returns JSON `null`.

## Sequence

1. `library.import` `{"paths":["/abs/path/photo.ARW"],"mode":"add"}`
   ```json
   {"duplicates":[],"failed":[],"imported":[1],"scanned":1,"sidecars":0}
   ```
   `imported` holds the new photo ids (u64). A file the engine cannot read goes to `failed` as `[path, reason]` and
   `imported` stays empty (the call still succeeds; path shortened here, the real one is the test temp dir):
   ```json
   {"duplicates":[],"failed":[["/tmp/broken.ARW","unrecognized file format"]],"imported":[],"scanned":1,"sidecars":0}
   ```
   Importing a path (`"reason":"path"`) or bytes (`"reason":"content"`) already in the library imports nothing and
   reports the existing photo (real output, path shortened):
   ```json
   {"duplicates":[{"existing":1,"path":"/tmp/fixture.png","reason":"path"}],"failed":[],"imported":[],"scanned":1,"sidecars":0}
   ```
   **Getting the photo id:** `imported[0]`, else `duplicates[0].existing`, else `restored[0]` (the engine's
   `restored` array, filled when a trashed photo is brought back by an `onDeleted` restore; not observed
   in the tests). Then call `library.select` with that id, so reopening a photo works like a first open.
   The engine already makes the first imported photo active.
2. `library.select` `{"ids":[1],"active":1}`
   ```json
   {"selected":1}
   ```
   Redundant after step 1, but it is the documented sequence for Swift (same as the engine's MCP tools), so keep it.
3. `develop.set` `{"control":"light.exposure","value":0.5}` returns `null`. Values outside a control's range are clamped, not rejected.

Control ids: `light.exposure`, `light.contrast`, `light.highlights`, `light.shadows`, `light.whites`, `light.blacks`,
`wb.temp` (Kelvin, e.g. 5000), `wb.tint`, `color.vibrance`, `color.saturation`.

## Parameterless check

`library.info` with `{}` or NULL returns a large object (library path, cache and persistence counters). Fields worth
reading: `photos` (count), `persistent`, `lastError`. Excerpt after the sequence above:

```json
{"albums":0,"lastError":null,"persistent":true,"photos":1,"seq":2,"unsavedError":null,"unsavedOps":0, "...":"..."}
```
