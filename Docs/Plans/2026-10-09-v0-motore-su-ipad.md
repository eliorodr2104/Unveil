# v0 — Motore su iPad: piano di implementazione

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** far girare il motore LightCraft (incluso nel repository, invariato, senza AI) dentro un'app UIKit minimale su iPad M2, e produrre la baseline: immagini di riferimento, misure, profilazione e mappa delle strutture dati.

**Architecture:** `Engine/` è un workspace Rust che contiene il motore upstream copiato al commit `c435d143de921e8dc724245065500bc191f91102` più il crate `ffi` (C ABI `uv_*`). Lo script di build produce `Frameworks/UnveilEngine.xcframework`. L'app Swift parla col motore solo tramite `EngineManager`; i frame arrivano in due `MTLBuffer` condivisi letti da un canvas Metal.

**Tech Stack:** Rust 1.97.1 (edition 2024), wgpu 30 (dentro il motore), Swift 6.2, UIKit + SwiftUI, Metal, iPadOS 26, Xcode 27, Swift Testing.

**Spec:** `Docs/Specs/2026-10-09-v0-motore-su-ipad-design.md`. È vincolante: in caso di conflitto vince la spec, e il conflitto si segnala al coordinatore.

## Global Constraints

- Deployment target **iPadOS 26.0**, solo iPad. Dispositivo di prova: iPad M2 dell'autore.
- Motore in `Engine/crates/`: **non si modifica nella v0.** L'unico file upstream modificabile è `Engine/Cargo.toml`, e solo nell'elenco `members`/`exclude` (più la rimozione delle righe `lightcraft-ui-egui` e `lightcraft-mcp` dalle `workspace.dependencies`, se Cargo lo richiede).
- `lightcraft-engine` **senza feature**: niente `sam`, `denoise`, `rawnind-model`. Mai chiamare `with_default_face_models` / `with_default_denoise_models`.
- `cargo` non è nel `PATH`: ogni comando Rust si esegue con `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`.
- Licenza: tutto il repository Apache-2.0. Nessun file in `docs/brand/` di upstream entra nel repository.
- C ABI: prefisso `uv_`, header `Engine/ffi/include/uv.h`, modulo Clang `UnveilEngine`.
- Commenti in inglese; `//` al massimo 2 righe; niente trattini lunghi (em-dash). `///` per la documentazione, nello stile antirez.
- Swift: 4 spazi, un tipo per file, allineamento in colonne, generici vincolati e `some` (niente `any` senza un commento che lo giustifichi), `Mutex` (Synchronization) per lo stato condiviso, niente actor, nessun `DispatchSemaphore` sul main thread.
- Rust (`Engine/ffi`): `rustfmt` con `max_width = 110`; ogni blocco `unsafe` ha un commento `// SAFETY:` (al massimo 2 righe). Niente `dyn` sul percorso critico.
- I worker **non fanno mai commit, stage o push**. Riportano ogni test eseguito con il risultato reale: un test non eseguito è un limite dichiarato, mai un successo.

## Review Focus

1. **L'app va in background durante un render.** Al ritorno la GPU deve essere ancora usata, senza ricadere per sempre sul processore. Il test Rust è in T6 (sospensione durante un render in corso); la prova sul dispositivo è in T13.
2. **Si apre un file corrotto o non supportato.** Deve comparire un errore leggibile, senza crash. Test Rust in T4 (file non valido → `UV_ERR_ENGINE` con messaggio); test Swift in T10 (`PhotoImporter` con un URL non leggibile).
3. **Trascinamento rapido dello slider.** L'ultimo frame mostrato deve corrispondere all'ultimo valore, non a uno intermedio. Test in T5: dieci richieste di fila; l'ultimo frame consegnato ha la generazione più alta ed è un render completo con il valore finale.
4. **Si apre una seconda foto mentre la prima è in render.** Un frame della prima foto non deve mai comparire dopo quelli della seconda. Test in T5 (risultato con foto non più attiva → scartato) e in T9 (`FrameSink` ignora generazioni più vecchie dell'ultima pubblicata).
5. **Chiusura della sessione con un render in corso.** Nessuna callback deve arrivare dopo il ritorno di `uv_session_free`. Test in T5.

---

## Modello di esecuzione

Questa sezione vale per il coordinatore. I worker ricevono solo il proprio ticket, le Global Constraints, la spec e `CODE_STYLE.md`.

- **Ruoli**
  - **Coordinatore** (la sessione principale): non implementa mai.
    - Crea il branch della milestone (`m1-bridge`, `m2-app`, `m3-baseline`) a partire da `main`, e un worktree per ogni sottoticket.
    - Sceglie modello ed effort di partenza.
    - Applica i sottoticket approvati come patch (`git diff` → `git apply --3way`).
    - Fa un commit per ticket sul branch della milestone, con `git commit -F`. Il messaggio ha il soggetto all'imperativo e il corpo in prosa che apre col problema e chiude con le evidenze. Niente trailer `Co-Authored-By` né attribuzioni a Claude, niente nomi di persone.
    - Il push lo fa l'autore.
  - **Planner:** Opus 5.5, effort high, in sola lettura. Divide il ticket in sottoticket. Ogni sottoticket ha file, criteri, comando di verifica e dipendenze. I sottoticket paralleli devono toccare file diversi.
  - **Worker:** implementano un sottoticket ciascuno, nel proprio worktree.
  - **Verifier:** Opus 5.5, effort high. Esegue lui il comando di verifica nel worktree e confronta il lavoro con spec, ticket e `CODE_STYLE.md`. Risponde `PASS`, `FAIL` (criteri o test) oppure `STYLE` (solo osservazioni di stile).
- **Scala in caso di `FAIL`:** Haiku 5.5 → Sonnet 5.5 low → medium → high → Opus 5.5 medium → high → xhigh → Fable 5.1 medium → high → max. Con `STYLE` si ripete il tentativo allo **stesso** livello, con le osservazioni allegate. La "partenza proposta" di ogni ticket è un'indicazione: decide il coordinatore.
- **Ci si ferma e si chiede all'autore** in questi casi: conflitto tra patch, `FAIL` a Fable max, stessa causa di fallimento due volte, ticket ambiguo, un 🛑 del piano, la fine di una milestone.
- **👤 Passaggi umani:** richiedono l'autore (iPad collegato, conferma dei download, prove al tatto). Il coordinatore si ferma e chiede.
- **Registro:** `.superpowers/sdd/2026-10-09-v0/progress.md`. Per ogni ticket: livello finale, tentativi, esito, comandi esatti.

---

# M1 — Bridge Rust

### Task 1: Repository, licenza, stile e motore incluso

**Partenza proposta:** Haiku 5.5. **Milestone:** M1.

**Files:**
- Create: `.gitignore`, `LICENSE`, `NOTICE`, `README.md`, `CLAUDE.md`, `CODE_STYLE.md`
- Create: `Engine/` (copia upstream), `Engine/UPSTREAM.md`
- Modify: `Engine/Cargo.toml` (solo `members`)

**Interfaces:**
- Consumes: niente.
- Produces: il workspace `Engine/` compilabile su Mac; `CODE_STYLE.md` per tutti i ticket seguenti.

- [ ] **Step 1: Inizializzare il repository**

```bash
cd /Users/mac/Personal_Projects/Unveil
git init -b main
```

`.gitignore`:

```gitignore
.DS_Store
xcuserdata/
DerivedData/
Engine/target/
/Frameworks/
/Baseline/
DerivedData*/
*.trace
.superpowers/sdd/*/worktrees/
```

- [ ] **Step 2: Copiare il motore al commit fissato**

```bash
TMP=$(mktemp -d)
git clone https://github.com/storytold/lightcraft "$TMP/lightcraft"
git -C "$TMP/lightcraft" checkout c435d143de921e8dc724245065500bc191f91102
mkdir -p Engine
rsync -a --exclude 'ui-egui' --exclude 'mcp' "$TMP/lightcraft/crates" Engine/
rsync -a "$TMP/lightcraft/assets" Engine/
cp "$TMP/lightcraft/"{Cargo.toml,Cargo.lock,rustfmt.toml,clippy.toml} Engine/
cp "$TMP/lightcraft/LICENSE-APACHE" LICENSE
cp "$TMP/lightcraft/NOTICE" "$TMP/upstream-NOTICE"
```

Expected: `Engine/crates/engine/Cargo.toml` esiste; `Engine/crates/ui-egui` non esiste; `ls Engine/assets` mostra `camera-profiles` e `fonts`.

- [ ] **Step 3: Restringere il workspace**

In `Engine/Cargo.toml` sostituire:

```toml
members = ["crates/*", "apps/*", "xtask"]
```

con:

```toml
members = ["crates/*"]
```

Il crate `ffi` verrà aggiunto in T2. Se al passo 4 Cargo segnala i percorsi mancanti di `lightcraft-ui-egui` o `lightcraft-mcp` in `[workspace.dependencies]`, rimuovere **solo** quelle due righe.

- [ ] **Step 4: Verificare che il motore compili sul Mac senza cambiare versioni**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd Engine && cargo build -p lightcraft-engine && cd ..
```

Expected: build OK. Il lockfile può perdere pacchetti (quelli di egui e MCP), ma **nessuna versione deve cambiare**: lo verifica lo Step 7, nella stessa shell (serve `$TMP`).

- [ ] **Step 5: Scrivere `Engine/UPSTREAM.md` e `NOTICE`**

`Engine/UPSTREAM.md`:

```markdown
# Upstream

Source: https://github.com/storytold/lightcraft
Commit: c435d143de921e8dc724245065500bc191f91102 (2026-10-09)
Used under the Apache License 2.0 option of upstream's "MIT OR Apache-2.0" dual license.

Copied: crates/ (without ui-egui and mcp), assets/, Cargo.toml, Cargo.lock, rustfmt.toml, clippy.toml.
Not copied: apps/, xtask/, docs/ (including the ArtCraft trademarks in docs/brand/), tools/, packaging/, nix/.
Local changes in v0: Engine/Cargo.toml `members` only. Every later change to a file under
crates/ is listed in CHANGES.md next to this file, as Apache-2.0 section 4(b) requires.
```

`NOTICE` (alla radice): una prima sezione

```text
Unveil
Copyright 2026 the Unveil authors. Licensed under the Apache License, Version 2.0 (LICENSE).

Engine/ contains LightCraft (https://github.com/storytold/lightcraft), commit c435d143,
used under the Apache License 2.0. Its NOTICE follows.
```

seguita dal contenuto integrale di `$TMP/upstream-NOTICE`.

- [ ] **Step 6: Scrivere `CODE_STYLE.md`, `CLAUDE.md` e `README.md`**

`CODE_STYLE.md` parte da una copia di `/Users/mac/Personal_Projects/Cascade/CODE_STYLE.md`, con queste modifiche:
- sostituire "Cascade" con "Unveil" nel titolo e nell'introduzione;
- riscrivere l'introduzione: editor RAW per iPad, UIKit + SwiftUI per i pannelli, motore Rust in `Engine/`, iPadOS 26 come minimo;
- eliminare le sezioni "Plugin SDK", "Host seams" e "Activity and notice contracts", e la sottosezione "The plugin engine";
- nel percorso critico, `InlineArray` e `Span` si usano **senza** controlli di versione (iPadOS 26);
- aggiungere una sezione **"Unveil additions"** con le regole della spec §7.2:
  - `//` al massimo 2 righe;
  - orientamento ai protocolli statico;
  - prima lo stack;
  - sul percorso critico vince il basso livello;
  - le regole Rust (`rustfmt` 110 solo per `Engine/ffi`, `// SAFETY:`, niente `dyn` sul percorso critico, codice upstream copiato non riformattato nella v0);
- nel "Folder layout" usare l'albero della spec §7.1.

`CLAUDE.md` (in inglese, al massimo 40 righe): cos'è il progetto, i link a spec, piano e `CODE_STYLE.md`, i comandi (`export PATH=…`, `cargo test -p unveil-ffi --features test-hooks`, `scripts/build-xcframework.sh`, `scripts/test-app.sh`), la regola "Engine/crates is upstream code: do not modify in v0", e la regola sui commit (solo il coordinatore).

`README.md`: tre righe (nome, cosa fa, licenza).

- [ ] **Step 7: Verificare le versioni nel lockfile**

```bash
git -C "$TMP/lightcraft" show HEAD:Cargo.lock > /tmp/upstream.lock
diff <(grep -A1 '^name = ' /tmp/upstream.lock | paste - - - | sort) \
     <(grep -A1 '^name = ' Engine/Cargo.lock | paste - - - | sort) | grep '^>' || echo "no version changes"
```

Expected: `no version changes` (sono ammesse solo righe `<`, cioè pacchetti rimossi).

- [ ] **Step 8: Il coordinatore fa il commit**

```bash
git add -A
git commit -F <messaggio>   # "Import the LightCraft engine at c435d14 and set up the repository"
```

---

### Task 2: Primo build per iOS 🛑

**Partenza proposta:** Sonnet 5.5 medium. **Milestone:** M1. **🛑 Punto di verifica:** se la build per `aarch64-apple-ios` fallisce dopo **un** tentativo di soluzione senza toccare `Engine/crates/`, il coordinatore si ferma e porta all'autore l'errore e le opzioni.

**Files:**
- Create: `Engine/ffi/Cargo.toml`, `Engine/ffi/rustfmt.toml`, `Engine/ffi/src/lib.rs`
- Modify: `Engine/Cargo.toml` (`members` += `"ffi"`)
- Create: `Docs/Baseline/build-ios.md`

**Interfaces:**
- Produces: crate `unveil-ffi` (lib `unveil_ffi`, `crate-type = ["staticlib", "rlib"]`); `uv_abi_version() -> u32` che restituisce `1`.

- [ ] **Step 1: Aggiungere i target**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

- [ ] **Step 2: Scrivere il crate minimo**

`Engine/ffi/Cargo.toml`:

```toml
[package]
name = "unveil-ffi"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
publish = false

[lib]
name = "unveil_ffi"
crate-type = ["staticlib", "rlib"]

[features]
# Exposes uv_test_panic for the bridge tests; never enabled in app builds.
test-hooks = []

[dependencies]
lightcraft-engine = { workspace = true }
lightcraft-gpu = { workspace = true }
serde_json = { workspace = true }
log = { workspace = true }

[dev-dependencies]
png = { workspace = true }

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"
```

`Engine/ffi/rustfmt.toml`:

```toml
max_width = 110
```

`Engine/ffi/src/lib.rs`:

```rust
//! unveil-ffi is the C ABI between the Unveil app and the LightCraft engine.

/// uv_abi_version lets the Swift side refuse a framework built for another header.
#[unsafe(no_mangle)]
pub extern "C" fn uv_abi_version() -> u32 {
    1
}
```

Aggiungere `"ffi"` a `members` in `Engine/Cargo.toml`. Se `png` non è in `[workspace.dependencies]`, usare la stessa versione che compare in `Engine/Cargo.lock`, con `png = "<versione>"`.

- [ ] **Step 3: Controllare l'albero delle dipendenze per iOS**

```bash
cd Engine
cargo tree -p unveil-ffi --target aarch64-apple-ios -e normal,build --prefix none \
  | sort -u | grep -E '^(ring|ash|cc|libfuzzer-sys|rav1e|built|harfrust|objc2-metal|wgpu) ' | tee /tmp/deps.txt
```

Expected: le righe trovate vanno registrate. `ash` (Vulkan) **non** dovrebbe esserci. Se c'è, va annotato e non si fa nulla: è una scelta di wgpu, da valutare nel sottoprogetto 2.

- [ ] **Step 4: Compilare per le tre piattaforme**

```bash
export IPHONEOS_DEPLOYMENT_TARGET=26.0
cargo build -p unveil-ffi --release --target aarch64-apple-ios
cargo build -p unveil-ffi --release --target aarch64-apple-ios-sim
cargo build -p unveil-ffi --release
ls -la target/aarch64-apple-ios/release/libunveil_ffi.a target/aarch64-apple-ios-sim/release/libunveil_ffi.a
```

Expected: le due `.a` esistono. Registrare le dimensioni.

- [ ] **Step 5: Ricavare le librerie native da collegare**

```bash
cargo rustc -p unveil-ffi --release --target aarch64-apple-ios --crate-type staticlib -- --print native-static-libs 2>&1 \
  | grep 'native-static-libs' | tee /tmp/native-libs.txt
```

Expected: una riga con i flag `-framework …` e `-l…`. Serve in T7.

- [ ] **Step 6: Scrivere `Docs/Baseline/build-ios.md`**

Il file contiene: comandi eseguiti, dipendenze del passo 3, dimensioni delle `.a`, riga `native-static-libs`, tempi di build e avvisi di compilazione rilevanti.

- [ ] **Step 7: Il coordinatore fa il commit** ("Build the engine for iPadOS behind an empty C ABI")

---

### Task 3: Base del bridge: header, stati, errori, panic, sessione

**Partenza proposta:** Opus 5.5 medium. **Milestone:** M1.

**Files:**
- Create: `Engine/ffi/include/uv.h`, `Engine/ffi/include/module.modulemap`
- Create: `Engine/ffi/src/status.rs`, `Engine/ffi/src/last_error.rs`, `Engine/ffi/src/panic_guard.rs`, `Engine/ffi/src/session.rs`
- Modify: `Engine/ffi/src/lib.rs`
- Test: `Engine/ffi/tests/bridge_core.rs`, `Engine/ffi/tests/common/mod.rs`

**Interfaces:**
- Produces (C, header completo, usato da tutti i ticket seguenti):

```c
// uv.h: C ABI of the Unveil engine bridge. Version 1.
#pragma once
#include <stdbool.h>
#include <stdint.h>

#define UV_ABI_VERSION 1

typedef enum UVStatus {
    UV_OK                    =  0,
    UV_ERR_INVALID_ARGUMENT  = -1,
    UV_ERR_UNKNOWN_COMMAND   = -2,
    UV_ERR_ENGINE            = -3,
    UV_ERR_PANIC             = -4,
    UV_ERR_SUSPENDED         = -5,
    UV_ERR_IO                = -6,
    UV_ERR_TIMEOUT           = -7,
} UVStatus;

typedef struct UVSession UVSession;

/// Called on the engine thread; `rgba` is valid only during the call.
typedef void (*uv_frame_cb)(void *ctx, const uint8_t *rgba, uint32_t width, uint32_t height,
                            uint32_t stride, uint64_t generation, bool draft);

uint32_t    uv_abi_version(void);
UVSession  *uv_session_new(const char *data_dir, uint64_t memory_budget);
void        uv_session_free(UVSession *session);

int32_t     uv_execute(UVSession *session, const char *command, const char *params_json,
                       char **result_json);
void        uv_string_free(char *string);
const char *uv_last_error(void);

uint64_t    uv_request_preview(UVSession *session, uint32_t max_pixels, bool draft,
                               uv_frame_cb callback, void *ctx);

int32_t     uv_suspend(UVSession *session);
int32_t     uv_resume(UVSession *session);
void        uv_set_memory_budget(UVSession *session, uint64_t bytes);
```

`module.modulemap`:

```text
module UnveilEngine {
    header "uv.h"
    export *
}
```

Firme Rust: `uv_abi_version` e `uv_last_error` sono `extern "C"` sicure; tutte le altre sono `unsafe extern "C"` (ricevono puntatori). Il parametro `callback` è `Option<UvFrameCb>` (`None` ⇔ `NULL`): una richiesta con `None` restituisce `0` con `UV_ERR_INVALID_ARGUMENT`, salvo che la sessione sia sospesa o senza foto attiva, che hanno la precedenza.

Rispetto alla spec ci sono tre precisazioni:
1. `uv_last_error` è **per thread** (come `errno`): vale fino alla chiamata `uv_*` successiva sullo stesso thread.
2. `uv_request_preview` restituisce la generazione, oppure `0` in caso di errore.
3. `uv_suspend`/`uv_resume` restituiscono uno stato.

- Produces (Rust):
  - `status::UVStatus` (`#[repr(i32)]`);
  - `last_error::set(&str)` e `last_error::clear()`;
  - `panic_guard::guard<T>(fallback: T, f: impl FnOnce() -> Result<T, Failure>) -> T`, dove `Failure { status: UVStatus, message: String }`;
  - `session::UVSession` con thread del motore e canale `Msg`.

- [ ] **Step 1: Scrivere i test che falliscono**

`Engine/ffi/tests/common/mod.rs`:

```rust
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use unveil_ffi::*;

/// TestSession owns a UVSession for one test and frees it on drop.
pub struct TestSession {
    pub raw: *mut UVSession,
    pub dir: tempdir_lite::Dir,
}

impl TestSession {
    pub fn new() -> TestSession {
        let dir = tempdir_lite::Dir::new();
        let path = CString::new(dir.path().to_str().unwrap()).unwrap();
        let raw = unsafe { uv_session_new(path.as_ptr(), 512 << 20) };
        assert!(!raw.is_null(), "uv_session_new failed: {}", last_error());
        TestSession { raw, dir }
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        unsafe { uv_session_free(self.raw) };
    }
}

pub fn last_error() -> String {
    let p = uv_last_error();
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// fixture_png writes a 96x64 RGB gradient the engine can import without any RAW file.
pub fn fixture_png(dir: &Path) -> PathBuf {
    let path = dir.join("fixture.png");
    let file = std::fs::File::create(&path).unwrap();
    let mut enc = png::Encoder::new(file, 96, 64);
    enc.set_color(png::ColorType::Rgb);
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> = (0..64).flat_map(|y| (0..96).flat_map(move |x| [x as u8 * 2, y as u8 * 3, 128])).collect();
    w.write_image_data(&data).unwrap();
    path
}

/// tempdir_lite avoids a tempfile dependency: a unique folder under the system temp dir.
pub mod tempdir_lite {
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Dir {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let p = std::env::temp_dir().join(format!("unveil-ffi-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &std::path::Path { &self.0 }
    }
    impl Drop for Dir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
}
```

`Engine/ffi/tests/bridge_core.rs`:

```rust
mod common;
use common::*;
use std::ffi::CString;
use unveil_ffi::*;

#[test]
fn abi_version_matches_header() {
    assert_eq!(uv_abi_version(), 1);
}

#[test]
fn session_opens_and_frees() {
    let _s = TestSession::new();
}

#[test]
fn null_data_dir_is_an_invalid_argument() {
    let raw = unsafe { uv_session_new(std::ptr::null(), 0) };
    assert!(raw.is_null());
    assert!(last_error().contains("data_dir"), "got: {}", last_error());
}

#[test]
fn string_free_accepts_null() {
    unsafe { uv_string_free(std::ptr::null_mut()) };
}

#[cfg(feature = "test-hooks")]
#[test]
fn panic_becomes_a_status_and_the_session_survives() {
    let s = TestSession::new();
    let status = unsafe { uv_test_panic(s.raw) };
    assert_eq!(status, UVStatus::UV_ERR_PANIC as i32);
    assert!(last_error().contains("test panic"), "got: {}", last_error());
    // The session still answers after a caught panic.
    let cmd = CString::new("engine.commands").unwrap();
    let mut out = std::ptr::null_mut();
    let status = unsafe { uv_execute(s.raw, cmd.as_ptr(), std::ptr::null(), &mut out) };
    assert_eq!(status, 0, "{}", last_error());
    unsafe { uv_string_free(out) };
}
```

Nota: `engine.commands` esiste nel protocollo di controllo. Se non è un comando di `Session::execute`, il worker usa il primo comando senza parametri elencato da `lightcraft_engine` (per esempio `library.memory`) e lo annota nel report. Lo stesso vale per tutti i test che usano `engine.commands`.

- [ ] **Step 2: Eseguire i test e verificare che falliscano**

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd Engine && cargo test -p unveil-ffi --features test-hooks
```

Expected: errori di compilazione (`uv_session_new` non definita).

- [ ] **Step 3: Implementare**

- **`status.rs`:**
  - l'`enum UVStatus` speculare a `uv.h`;
  - `Failure::engine(e)` mappa i messaggi che iniziano con `unknown command` a `UV_ERR_UNKNOWN_COMMAND` e tutti gli altri a `UV_ERR_ENGINE`.
- **`last_error.rs`:**
  - `thread_local! { static LAST: RefCell<Option<CString>> }`;
  - `uv_last_error` restituisce il puntatore o `null`;
  - un messaggio con byte nulli viene ripulito sostituendoli con `?`.
- **`panic_guard.rs`:**

```rust
/// guard runs one exported call: a panic becomes UV_ERR_PANIC with its message, never an unwind
/// across the C boundary, which would abort the process.
pub fn guard<T>(fallback: T, f: impl FnOnce() -> Result<T, Failure>) -> T {
    last_error::clear();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(failure)) => {
            last_error::set(&failure.message);
            fallback_for(failure.status, fallback)
        }
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            last_error::set(&format!("panic: {msg}"));
            fallback
        }
    }
}
```

  `fallback_for` serve alle funzioni che restituiscono `i32`. Il worker può sostituirla con un trait `FromStatus` implementato per `i32`, `u64` e i puntatori. Il vincolo è uno solo: ogni funzione esportata passa da `guard`.

- **`session.rs`:**
  - `UVSession { tx: mpsc::Sender<Msg>, thread: Option<JoinHandle<()>> }`.
  - Il thread si chiama `unveil-engine`, ha stack di 8 MiB e imposta la QoS `USER_INITIATED` con `pthread_set_qos_class_self_np` (`extern "C"`, esiste su iOS e macOS).
  - Il thread crea `lightcraft_engine::Session::new()`, applica `.with_fs()` se esiste su `Session` e chiama `open_library(data_dir, false)`.
  - Poi riceve `Msg::Execute { command, params, reply }` e `Msg::Shutdown`.
  - `uv_session_free` invia `Shutdown` e fa `join`.
  - `#[cfg(feature = "test-hooks")] uv_test_panic` esegue `panic!("test panic")` dentro `guard`.

- **`uv_execute`:**
  - valida i puntatori (`command` non nullo, `result_json` non nullo);
  - `params_json` nullo vale `{}`;
  - invia `Execute` e attende con `recv_timeout(30 s)`, che alla scadenza dà `UV_ERR_TIMEOUT`;
  - scrive in `*result_json` una `CString` allocata da Rust, che si libera con `uv_string_free` (`CString::from_raw`).

- [ ] **Step 4: Eseguire i test**

```bash
cd Engine && cargo test -p unveil-ffi --features test-hooks && cargo fmt -p unveil-ffi -- --check && cargo clippy -p unveil-ffi --features test-hooks -- -D warnings
```

Expected: tutti `PASS`, fmt pulito, clippy senza avvisi.

- [ ] **Step 5: Test di perdita di memoria (spec §6.5)**

`Engine/ffi/tests/leaks.rs` è un binario di test a sé, perché sostituisce l'allocatore globale. `dhat` è già in `[workspace.dependencies]`: va aggiunto ai `[dev-dependencies]` con `dhat = { workspace = true }`.

```rust
mod common;
use common::*;
use std::ffi::CString;
use unveil_ffi::*;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[test]
fn execute_results_and_errors_do_not_leak() {
    let s = TestSession::new();
    let warm = |n: usize| {
        for _ in 0..n {
            let cmd = CString::new("engine.commands").unwrap();
            let mut out = std::ptr::null_mut();
            unsafe { uv_execute(s.raw, cmd.as_ptr(), std::ptr::null(), &mut out) };
            unsafe { uv_string_free(out) };
            let bad = CString::new("no.such.command").unwrap();
            unsafe { uv_execute(s.raw, bad.as_ptr(), std::ptr::null(), &mut out) };
            let _ = last_error();
        }
    };
    warm(10);
    let _profiler = dhat::Profiler::builder().testing().build();
    let before = dhat::HeapStats::get();
    warm(1000);
    let after = dhat::HeapStats::get();
    // A few blocks of slack for lazily grown caches; a leak would add >= 1000.
    assert!(after.curr_blocks <= before.curr_blocks + 16, "{before:?} -> {after:?}");
}
```

Comando: `cargo test -p unveil-ffi --features test-hooks --test leaks`. Expected: `PASS`.

- [ ] **Step 6: Il coordinatore fa il commit** ("Add the C ABI core: status codes, per-thread errors, panic guard and engine thread")

---

### Task 4: Comandi del motore: importare, selezionare, regolare

**Partenza proposta:** Sonnet 5.5 high. **Milestone:** M1.

**Files:**
- Modify: `Engine/ffi/src/session.rs`
- Test: `Engine/ffi/tests/commands.rs`
- Create: `Docs/Baseline/engine-commands.md`

**Interfaces:**
- Consumes: T3.
- Produces: la sequenza documentata che Swift userà in T9:
  - `library.import` `{"paths":[path],"mode":"add"}` → risultato con l'id della foto;
  - `library.select` `{"ids":[id],"active":id}`;
  - `develop.set` `{"control":"light.exposure","value":0.5}`.

  Gli id dei controlli sono `light.exposure`, `light.contrast`, `light.highlights`, `light.shadows`, `light.whites`, `light.blacks`, `wb.temp`, `wb.tint`, `color.vibrance`, `color.saturation`.

- [ ] **Step 1: Scrivere i test**

`Engine/ffi/tests/commands.rs`:

```rust
mod common;
use common::*;
use std::ffi::{CStr, CString};
use unveil_ffi::*;

fn exec(s: &TestSession, cmd: &str, params: &str) -> Result<serde_json::Value, (i32, String)> {
    let c = CString::new(cmd).unwrap();
    let p = CString::new(params).unwrap();
    let mut out = std::ptr::null_mut();
    let status = unsafe { uv_execute(s.raw, c.as_ptr(), p.as_ptr(), &mut out) };
    if status != 0 {
        return Err((status, last_error()));
    }
    let json = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_owned();
    unsafe { uv_string_free(out) };
    Ok(serde_json::from_str(&json).unwrap_or(serde_json::Value::Null))
}

/// import_fixture imports the gradient PNG and makes it the active photo; returns its id.
pub fn import_fixture(s: &TestSession) -> u64 {
    let path = fixture_png(s.dir.path());
    let r = exec(s, "library.import", &format!(r#"{{"paths":["{}"],"mode":"add"}}"#, path.display())).unwrap();
    let id = photo_id_from_import(&r);
    exec(s, "library.select", &format!(r#"{{"ids":[{id}],"active":{id}}}"#)).unwrap();
    id
}

#[test]
fn import_select_and_set_exposure() {
    let s = TestSession::new();
    import_fixture(&s);
    exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.5}"#).unwrap();
}

#[test]
fn every_basic_control_is_accepted() {
    let s = TestSession::new();
    import_fixture(&s);
    for id in ["light.exposure", "light.contrast", "light.highlights", "light.shadows", "light.whites",
               "light.blacks", "wb.temp", "wb.tint", "color.vibrance", "color.saturation"] {
        let value = if id == "wb.temp" { 5000.0 } else { 10.0 };
        exec(&s, "develop.set", &format!(r#"{{"control":"{id}","value":{value}}}"#))
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
}

#[test]
fn unknown_command_has_its_own_status() {
    let s = TestSession::new();
    let (status, msg) = exec(&s, "no.such.command", "{}").unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_UNKNOWN_COMMAND as i32, "{msg}");
}

#[test]
fn a_file_that_is_not_an_image_is_an_engine_error_not_a_crash() {
    let s = TestSession::new();
    let bad = s.dir.path().join("broken.ARW");
    std::fs::write(&bad, b"this is not a raw file").unwrap();
    let r = exec(&s, "library.import", &format!(r#"{{"paths":["{}"],"mode":"add"}}"#, bad.display()));
    // Either the import is refused, or it yields no photo; both must leave the session usable.
    if let Ok(v) = r { assert!(photo_ids_from_import(&v).is_empty(), "imported garbage: {v}"); }
    exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.1}"#).unwrap_err();
}

#[test]
fn malformed_json_is_an_invalid_argument() {
    let s = TestSession::new();
    let (status, _) = exec(&s, "develop.set", "{not json").unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_INVALID_ARGUMENT as i32);
}
```

`photo_id_from_import` e `photo_ids_from_import` vanno in `common/mod.rs`. Leggono la forma reale del risultato di `library.import`: il worker la ricava eseguendo il comando una volta e la documenta in `Docs/Baseline/engine-commands.md`. Nell'ultimo test `develop.set` deve fallire perché non c'è una foto attiva: quel caso si risolve con `has_active` nel motore.

- [ ] **Step 2: Eseguire e verificare che falliscano**

`cd Engine && cargo test -p unveil-ffi --features test-hooks --test commands`. Expected: `FAIL`, perché le funzioni di supporto (`photo_id_from_import`, `photo_ids_from_import`) mancano, oppure perché `UV_ERR_INVALID_ARGUMENT` non viene ancora restituito per il JSON non valido.

- [ ] **Step 3: Implementare**

Nel thread del motore, `Execute` fa questo:
1. parsa `params` con `serde_json`; se il JSON non è valido restituisce `UV_ERR_INVALID_ARGUMENT`;
2. chiama `session.execute(command, &params)`;
3. serializza il `Value` risultante;
4. mappa gli errori con `Failure::engine`.

Aggiungere le due funzioni di supporto a `common/mod.rs` e scrivere `Docs/Baseline/engine-commands.md`: comandi, parametri e forme reali dei risultati.

- [ ] **Step 4: Eseguire tutti i test, fmt e clippy** (stessi comandi di T3, Step 4). Expected: `PASS`.

- [ ] **Step 5: Il coordinatore fa il commit** ("Route engine commands through uv_execute")

---

### Task 5: Thread di render e PreviewScheduler

**Partenza proposta:** Opus 5.5 high. **Milestone:** M1.

**Files:**
- Create: `Engine/ffi/src/preview_scheduler.rs`, `Engine/ffi/src/render_worker.rs`
- Modify: `Engine/ffi/src/session.rs`, `Engine/ffi/src/lib.rs`
- Test: unit test in `preview_scheduler.rs`; `Engine/ffi/tests/preview.rs`

**Interfaces:**
- Consumes: T3, T4 (`import_fixture` in `tests/common`; spostarla lì da `commands.rs`).
- Produces: `uv_request_preview` funzionante. Callback sul thread `unveil-engine`, `stride = width * 4`, RGBA8 sRGB come lo produce `Rendered::image`.

**Comportamento richiesto:**
- `uv_request_preview` incrementa un `AtomicU64` (la prima generazione è 1) e invia `Msg::Preview { generation, max_pixels, draft, callback }`. Restituisce subito la generazione.
- Il thread del motore usa `PreviewScheduler`, che è puro e testabile senza motore:

```rust
/// PreviewScheduler keeps at most one render in flight and remembers only the newest request:
/// a slider fires faster than the engine renders, and every older value is already stale.
pub struct PreviewScheduler<R> {
    in_flight     : Option<u64>,
    pending       : Option<R>,
    last_delivered: u64,
}

impl<R: Request> PreviewScheduler<R> {
    pub fn new() -> Self;
    /// Returns the request to start now, or None when one is already rendering (it becomes pending).
    pub fn submit(&mut self, request: R) -> Option<R>;
    /// Marks `generation` finished; returns the pending request to start next, if any.
    pub fn finished(&mut self, generation: u64) -> Option<R>;
    /// True when a finished render should reach the callback: newer than anything delivered, and
    /// still for the active photo.
    pub fn should_deliver(&mut self, generation: u64, rendered_photo: u64, active_photo: Option<u64>) -> bool;
    /// Drops the pending request (used by suspend and shutdown).
    pub fn clear_pending(&mut self);
    pub fn is_idle(&self) -> bool;
}

pub trait Request { fn generation(&self) -> u64; }
```

- **Avvio di una richiesta:** il thread del motore costruisce il job con `session.render_job(active, max, max, false, true)` e applica `.draft()` se richiesto. Poi lo invia a `render_worker`, un thread `unveil-render` con stack di 8 MiB e QoS `USER_INITIATED`, che esegue `job.run()` e rimanda `Msg::RenderDone { generation, draft, photo, result, callback }`.
- **Arrivo di `RenderDone`:**
  1. `session.accept(&result)`;
  2. se `should_deliver` e `result.rendered` è `Ok`, si invoca la callback sul thread del motore con i pixel di `rendered.image`;
  3. infine `finished(generation)` avvia l'eventuale richiesta in attesa.
- **Nessuna foto attiva:** `uv_request_preview` restituisce `0` con `UV_ERR_ENGINE` e il messaggio "no active photo".
- **`Shutdown`:** svuota `pending`, aspetta il `RenderDone` del render in corso (al massimo 10 s), chiude il worker e ne fa `join`. **Dopo il ritorno di `uv_session_free` non arriva più nessuna callback.**
- **Callback:** è `struct FrameCallback { f: uv_frame_cb, ctx: *mut c_void }` con `unsafe impl Send`, giustificata da un `// SAFETY:` nel codice: chi chiama garantisce che `ctx` resti valido e usabile da un altro thread finché la sessione è viva.

- [ ] **Step 1: Unit test del scheduler (in `preview_scheduler.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, PartialEq)]
    struct R(u64);
    impl Request for R { fn generation(&self) -> u64 { self.0 } }

    #[test]
    fn idle_scheduler_starts_immediately() {
        let mut s = PreviewScheduler::new();
        assert_eq!(s.submit(R(1)), Some(R(1)));
    }

    #[test]
    fn requests_during_a_render_keep_only_the_newest() {
        let mut s = PreviewScheduler::new();
        s.submit(R(1));
        for g in 2..=10 { assert_eq!(s.submit(R(g)), None); }
        assert_eq!(s.finished(1), Some(R(10)));
        assert_eq!(s.finished(10), None);
        assert!(s.is_idle());
    }

    #[test]
    fn older_or_foreign_results_are_not_delivered() {
        let mut s: PreviewScheduler<R> = PreviewScheduler::new();
        assert!(s.should_deliver(5, 7, Some(7)));
        assert!(!s.should_deliver(4, 7, Some(7)), "older than delivered");
        assert!(!s.should_deliver(6, 7, Some(8)), "photo no longer active");
        assert!(!s.should_deliver(7, 7, None), "no active photo");
    }

    #[test]
    fn clear_pending_drops_the_waiting_request() {
        let mut s = PreviewScheduler::new();
        s.submit(R(1));
        s.submit(R(2));
        s.clear_pending();
        assert_eq!(s.finished(1), None);
    }
}
```

- [ ] **Step 2: Test d'integrazione (`tests/preview.rs`)**

```rust
mod common;
use common::*;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use unveil_ffi::*;

#[derive(Default)]
struct Frames { list: Mutex<Vec<(u64, bool, u32, u32, u32, usize)>>, cv: Condvar }

extern "C" fn record(ctx: *mut std::ffi::c_void, rgba: *const u8, w: u32, h: u32, stride: u32, g: u64, draft: bool) {
    let frames = unsafe { &*(ctx as *const Frames) };
    assert!(!rgba.is_null());
    let bytes = unsafe { std::slice::from_raw_parts(rgba, (stride * h) as usize) };
    frames.list.lock().unwrap().push((g, draft, w, h, stride, bytes.iter().filter(|b| **b != 0).count()));
    frames.cv.notify_all();
}

fn wait_for(frames: &Frames, n: usize) -> Vec<(u64, bool, u32, u32, u32, usize)> {
    let list = frames.list.lock().unwrap();
    let (list, _) = frames.cv.wait_timeout_while(list, Duration::from_secs(30), |l| l.len() < n).unwrap();
    list.clone()
}

#[test]
fn a_preview_arrives_with_sane_geometry() {
    let s = TestSession::new();
    import_fixture(&s);
    let frames = Arc::new(Frames::default());
    let g = unsafe { uv_request_preview(s.raw, 512, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    assert!(g >= 1, "{}", last_error());
    let got = wait_for(&frames, 1);
    let (gen, draft, w, h, stride, nonzero) = got[0];
    assert_eq!((gen, draft), (g, false));
    assert!(w > 0 && h > 0 && w <= 512 && h <= 512);
    assert_eq!(stride, w * 4);
    assert!(nonzero > 0, "image is all zeros");
}

#[test]
fn a_burst_ends_with_the_newest_generation() {
    let s = TestSession::new();
    import_fixture(&s);
    let frames = Arc::new(Frames::default());
    let mut last = 0;
    for i in 0..10 {
        exec_ok(&s, "develop.set", &format!(r#"{{"control":"light.exposure","value":{}}}"#, i as f64 / 10.0));
        last = unsafe { uv_request_preview(s.raw, 256, i < 9, Some(record), Arc::as_ptr(&frames) as *mut _) };
    }
    std::thread::sleep(Duration::from_secs(3));
    let got = frames.list.lock().unwrap().clone();
    assert!(!got.is_empty());
    assert!(got.windows(2).all(|w| w[0].0 < w[1].0), "generations must increase: {got:?}");
    let final_frame = got.last().unwrap();
    assert_eq!((final_frame.0, final_frame.1), (last, false), "last frame must be the final full render");
}

#[test]
fn no_callback_after_free_returns() {
    let frames = Arc::new(Frames::default());
    {
        let s = TestSession::new();
        import_fixture(&s);
        unsafe { uv_request_preview(s.raw, 2048, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    } // uv_session_free runs here, with the render likely still in flight
    let n = frames.list.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames.list.lock().unwrap().len(), n);
}

#[test]
fn preview_without_an_active_photo_is_an_error() {
    let s = TestSession::new();
    let g = unsafe { uv_request_preview(s.raw, 256, false, Some(record), std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("no active photo"), "{}", last_error());
}
```

Spostare `exec` in `common/mod.rs` e aggiungere `exec_ok(&s, cmd, params)` (lo stesso, ma con `unwrap`).

- [ ] **Step 3: Verificare che falliscano**

`cd Engine && cargo test -p unveil-ffi --features test-hooks`. Expected: errori di compilazione o `FAIL`.

- [ ] **Step 4: Implementare** seguendo il "Comportamento richiesto".

- [ ] **Step 5: Eseguire i test 3 volte di fila**, per scoprire test instabili legati ai tempi:

```bash
cd Engine && for i in 1 2 3; do cargo test -p unveil-ffi --features test-hooks || exit 1; done
cargo fmt -p unveil-ffi -- --check && cargo clippy -p unveil-ffi --features test-hooks -- -D warnings
```

Expected: tre `PASS` di fila.

- [ ] **Step 6: Il coordinatore fa il commit** ("Render previews on a worker thread, keeping only the newest request")

---

### Task 6: Memoria, sospensione e ripresa

**Partenza proposta:** Sonnet 5.5 high. **Milestone:** M1.

**Files:**
- Create: `Engine/ffi/src/memory_budget.rs`
- Modify: `Engine/ffi/src/session.rs`, `Engine/ffi/src/lib.rs`
- Test: `Engine/ffi/tests/lifecycle.rs`

**Interfaces:**
- Produces:
  - `uv_set_memory_budget(bytes)` → `lightcraft_engine::memory::set_budget(bytes as usize)`;
  - `uv_suspend` → `UV_OK` quando nessun render è più in corso, `UV_ERR_TIMEOUT` dopo 2 s;
  - `uv_resume` → `lightcraft_gpu::reset_failures()` e accetta di nuovo le anteprime.
- In `uv_session_new` si applica il budget passato e si registra il release hook:

```rust
unsafe extern "C" {
    // libSystem on iOS and macOS: returns freed pages to the system after a cache trim.
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

fn release_memory() {
    // SAFETY: a null zone means "all zones"; goal 0 means "as much as possible".
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}
```

  Poi si chiama `lightcraft_engine::memory::set_release_hook(release_memory)`. Se il modulo `memory` o `reset_failures` non sono pubblici, il worker si ferma e lo segnala: **nella v0 non si modifica `Engine/crates/`**.

- [ ] **Step 1: Test**

```rust
mod common;
use common::*;
use std::sync::Arc;
use unveil_ffi::*;

#[test]
fn suspended_session_refuses_previews_and_resume_restores_them() {
    let s = TestSession::new();
    import_fixture(&s);
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    let g = unsafe { uv_request_preview(s.raw, 256, false, None, std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("suspended"), "{}", last_error());
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    let frames = Arc::new(Frames::default());
    let g = unsafe { uv_request_preview(s.raw, 256, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    assert!(g > 0);
    assert_eq!(wait_for(&frames, 1)[0].0, g);
}

#[test]
fn suspend_waits_for_the_render_in_flight() {
    let s = TestSession::new();
    import_fixture(&s);
    let frames = Arc::new(Frames::default());
    unsafe { uv_request_preview(s.raw, 2048, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0);
    // After suspend returns nothing renders: a late frame here would be GPU work in background.
    let n = frames.list.lock().unwrap().len();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(frames.list.lock().unwrap().len(), n);
}

#[test]
fn commands_still_work_while_suspended() {
    let s = TestSession::new();
    import_fixture(&s);
    unsafe { uv_suspend(s.raw) };
    exec_ok(&s, "develop.set", r#"{"control":"light.exposure","value":0.3}"#);
}

#[test]
fn memory_budget_is_applied() {
    let s = TestSession::new();
    unsafe { uv_set_memory_budget(s.raw, 300 << 20) };
    assert_eq!(lightcraft_engine::memory::budget(), 300 << 20);
}
```

`Frames`, `record` e `wait_for` vanno spostati in `tests/common/mod.rs`. `lightcraft-engine` va aggiunto ai `[dev-dependencies]` per l'ultimo test: è già una dipendenza, quindi basta `use`.

Nota: `memory::budget()` è globale al processo. I test che lo modificano vanno eseguiti con `--test-threads=1` se interferiscono tra loro. Se è così, il worker lo annota e aggiunge `serial` a mano con un `static Mutex<()>`.

- [ ] **Step 2: Verificare che falliscano, implementare, rieseguire** (stessi comandi di T5, Step 5). Expected: `PASS` tre volte.

- [ ] **Step 3: Il coordinatore fa il commit** ("Let the host set the memory budget and pause GPU work in background")

---

### Task 7: XCFramework e impostazioni di link

**Partenza proposta:** Sonnet 5.5 medium. **Milestone:** M1.

**Files:**
- Create: `scripts/build-xcframework.sh`, `Config/UnveilEngine.xcconfig` (generato dallo script e committato)

**Interfaces:**
- Produces: `Frameworks/UnveilEngine.xcframework` (ignorato da git) con le slice `ios-arm64` e `ios-arm64-simulator`, header e module map. `Config/UnveilEngine.xcconfig` contiene `OTHER_LDFLAGS` ricavati da `--print native-static-libs`.
- Opzione `--cpu apple-m1`, che aggiunge `-C target-cpu=apple-m1` (serve al confronto A/B di T15).

- [ ] **Step 1: Scrivere lo script**

```bash
#!/bin/bash
# Builds the engine bridge for iPad and the simulator and packs it as an XCFramework.
set -euo pipefail
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export IPHONEOS_DEPLOYMENT_TARGET=26.0
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CPU=""
[[ "${1:-}" == "--cpu" ]] && CPU="${2:?usage: --cpu <name>}"
FLAGS=""
[[ -n "$CPU" ]] && FLAGS="-C target-cpu=$CPU"

cd "$ROOT/Engine"
for T in aarch64-apple-ios aarch64-apple-ios-sim; do
    RUSTFLAGS="$FLAGS" cargo build -p unveil-ffi --release --target "$T"
done

LIBS=$(RUSTFLAGS="$FLAGS" cargo rustc -p unveil-ffi --release --target aarch64-apple-ios \
    --crate-type staticlib -- --print native-static-libs 2>&1 | sed -n 's/.*native-static-libs: //p' | tail -1)
mkdir -p "$ROOT/Config"
printf '// Generated by scripts/build-xcframework.sh. Do not edit.\nOTHER_LDFLAGS = $(inherited) %s\n' "$LIBS" \
    > "$ROOT/Config/UnveilEngine.xcconfig"

OUT="$ROOT/Frameworks/UnveilEngine.xcframework"
rm -rf "$OUT"
xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libunveil_ffi.a     -headers ffi/include \
    -library target/aarch64-apple-ios-sim/release/libunveil_ffi.a -headers ffi/include \
    -output "$OUT"
echo "built $OUT${CPU:+ (target-cpu=$CPU)}"
```

- [ ] **Step 2: Eseguire e controllare**

```bash
chmod +x scripts/build-xcframework.sh && scripts/build-xcframework.sh
ls Frameworks/UnveilEngine.xcframework/ios-arm64/Headers Frameworks/UnveilEngine.xcframework/ios-arm64-simulator/Headers
cat Config/UnveilEngine.xcconfig
nm -gU Frameworks/UnveilEngine.xcframework/ios-arm64/libunveil_ffi.a | grep -E ' _uv_' | sort -u
```

Expected:
- in ogni cartella `Headers` ci sono `uv.h` e `module.modulemap`;
- l'xcconfig contiene `-framework Metal` (o simile);
- `nm` elenca esattamente i 12 simboli `_uv_` di `uv.h`, senza `_uv_test_panic`.

- [ ] **Step 3: Il coordinatore fa il commit** ("Pack the bridge as an XCFramework with its link flags")

**🛑 Fine M1.** Il coordinatore si ferma e consegna all'autore:
- riepilogo dei ticket con livelli e tentativi;
- `git log m1-bridge`;
- conferma che `cargo test` è verde.

L'autore fa il push. Gli si chiede anche se è il momento di creare il repository GitHub pubblico.

---

# M2 — App iPad

### Task 8: 👤 Progetto Xcode

**Partenza proposta:** passaggio umano, poi Sonnet 5.5 medium. **Milestone:** M2.

**Passaggio umano.** È umano perché la firma richiede il tuo account; il resto si fa in 5 minuti nell'interfaccia di Xcode, ed è più affidabile che scrivere a mano un `project.pbxproj`.

1. Xcode → File → New → Project → **iOS App**:
   - Product Name `Unveil`, Organization Identifier a scelta (il bundle id diventa `<org>.Unveil`);
   - Interface **Storyboard**, Language **Swift**, Testing System **Swift Testing**, **Include Tests** attivo;
   - percorso `/Users/mac/Personal_Projects/Unveil`, senza "Create Git repository".
2. Target Unveil → General:
   - Supported Destinations: solo **iPad** (rimuovere iPhone e Mac);
   - Minimum Deployments **iPadOS 26.0**.
3. Signing & Capabilities:
   - scegliere il Team;
   - **+ Capability**: *Increased Memory Limit* ed *Extended Virtual Addressing*. Se una delle due non è disponibile con il team personale, annotarlo e proseguire: il coordinatore aggiorna la spec.
4. Trascinare `Frameworks/UnveilEngine.xcframework` (eseguire prima `scripts/build-xcframework.sh`) nel target, con **Do Not Embed**.
5. Project → Info → Configurations: per Debug e Release del target Unveil, impostare `Config/UnveilEngine.xcconfig` come file di configurazione.
6. Chiudere Xcode.

**Poi il worker:**

**Files:**
- Delete: `Unveil/Main.storyboard`, `Unveil/ContentView.swift` (se presente)
- Create/Modify: `Unveil/App/AppDelegate.swift`, `Unveil/App/SceneDelegate.swift`, `Unveil/Info.plist` (nessun `UIMainStoryboardFile`, nessun `UISceneStoryboardFile`)
- Create: `scripts/test-app.sh`
- Create: le cartelle vuote della spec §7.1 (`Core/Engine`, `Core/Memory`, …) con un `.gitkeep`

- [ ] **Step 1: Verificare che il progetto usi cartelle sincronizzate**

```bash
grep -c PBXFileSystemSynchronizedRootGroup Unveil.xcodeproj/project.pbxproj
```

Expected: ≥ 1. Se è 0, fermarsi e segnalarlo: senza cartelle sincronizzate ogni file nuovo tocca il `pbxproj` e il parallelismo salta.

- [ ] **Step 2: `SceneDelegate` crea la finestra senza storyboard** e mostra un `UIViewController` con sfondo `.systemBackground` e una label "Unveil". L'editor arriva in T12.

- [ ] **Step 3: `scripts/test-app.sh`**

```bash
#!/bin/bash
# Runs the app's unit tests on the first available iPad simulator.
set -euo pipefail
SIM=$(xcrun simctl list devices available | grep -m1 -oE 'iPad[^()]+\(([0-9A-F-]{36})\)' | grep -oE '[0-9A-F-]{36}')
xcodebuild test -project Unveil.xcodeproj -scheme Unveil -destination "id=$SIM" \
    -derivedDataPath "${DERIVED_DATA:-DerivedData}" -quiet
```

- [ ] **Step 4: Test di fumo**, in `UnveilTests/EngineLinkTests.swift`:

```swift
//
//  EngineLinkTests.swift
//  Unveil
//

import Testing
import UnveilEngine

struct EngineLinkTests {

    @Test
    func abiVersionMatchesHeader() {
        #expect(uv_abi_version() == UInt32(UV_ABI_VERSION))
    }
}
```

- [ ] **Step 5: Eseguire**

```bash
scripts/test-app.sh
```

Expected: `PASS`. Un errore di link (simbolo mancante di Metal, `objc` o `c++`) significa che l'xcconfig non è applicato.

- [ ] **Step 6: Il coordinatore fa il commit** ("Create the iPad app shell linked to the engine")

---

### Task 9: EngineManager, EngineError, FrameSink

**Partenza proposta:** Opus 5.5 high. **Milestone:** M2.

**Files:**
- Create: `Unveil/Core/Protocols/EngineDriving.swift`, `Unveil/Core/Engine/EngineManager.swift`, `Unveil/Core/Engine/FrameSink.swift`, `Unveil/Core/Errors/EngineError.swift`, `Unveil/Models/Engine/PhotoID.swift`, `Unveil/Models/Engine/ReadyFrame.swift`, `Unveil/Models/Engine/Enums/DevelopAdjustmentKind.swift`
- Test: `UnveilTests/EngineErrorTests.swift`, `UnveilTests/FrameSinkTests.swift`, `UnveilTests/EngineManagerTests.swift`

**Interfaces:**
- Produces:

```swift
protocol EngineDriving: AnyObject, Sendable {

    var frames: FrameSink { get }

    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID
    func set(_ adjustment: DevelopAdjustmentKind, to value: Double) async throws(EngineError)
    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64
    func suspend() throws(EngineError)
    func resume() throws(EngineError)
    func setMemoryBudget(bytes: UInt64)
}

enum EngineError: Error, Equatable {
    case invalidArgument(String)
    case unknownCommand(String)
    case engine(String)
    case panic(String)
    case suspended(String)
    case io(String)
    case timeout(String)
    case unknown(code: Int32, message: String)

    init(status: Int32, message: String)   // UV_* → case; unknown codes → .unknown
}

enum DevelopAdjustmentKind: String, CaseIterable, Sendable {
    case exposure   = "light.exposure"
    case contrast   = "light.contrast"
    case highlights = "light.highlights"
    case shadows    = "light.shadows"
    case whites     = "light.whites"
    case blacks     = "light.blacks"
    case temperature = "wb.temp"
    case tint       = "wb.tint"
    case vibrance   = "color.vibrance"
    case saturation = "color.saturation"
    // range, defaultValue, step from Engine/crates/develop/src/controls.rs
    var range       : ClosedRange<Double> { get }
    var defaultValue: Double { get }
}

struct PhotoID: Hashable, Sendable { let rawValue: UInt64 }

/// ReadyFrame describes the newest frame a FrameSink holds; the canvas reads it, never the pixels' owner.
struct ReadyFrame: Equatable, Sendable {
    let bufferIndex : Int
    let width       : Int
    let height      : Int
    let bytesPerRow : Int
    let generation  : UInt64
    let isDraft     : Bool
}

final class FrameSink: Sendable {

    init(device: some MTLDevice, maxPixels: Int)          // two shared MTLBuffers, sized once
    var buffers: (any MTLBuffer, any MTLBuffer) { get }    // `any` here: Metal hands out existentials
    func latest() -> ReadyFrame?
    var onFrame: (@Sendable () -> Void)? { get set }       // wakes the canvas; set once at startup
    /// Copies one engine frame into the buffer not currently published. Called on the engine thread.
    func receive(rgba: UnsafeRawPointer, width: Int, height: Int, stride: Int,
                 generation: UInt64, isDraft: Bool)
}
```

**Regole di `FrameSink`:**
- **`bytesPerRow` allineato:** vale `alignUp(width * 4, device.minimumLinearTextureAlignment(for: .rgba8Unorm))`. La copia avviene riga per riga, perché la stride del motore (`width * 4`) può non essere allineata.
- **Frame obsoleti:** un frame con `generation <=` dell'ultimo pubblicato viene ignorato.
- **Stato condiviso:** sta in un `Mutex<State>` (`import Synchronization`), con l'indice del prossimo buffer, l'ultimo `ReadyFrame` e `onFrame`.
- **Notifica:** dopo la pubblicazione si chiama `onFrame`, fuori dal lock.
- **Frame troppo grandi:** se `width * height` supera la capacità, il frame viene ignorato e registrato con `os.Logger`.

**`EngineManager`:**
- possiede l'handle `OpaquePointer` di `uv_session_new`, con `dataDirectory` = `Application Support/Engine`;
- esegue tutte le chiamate `uv_execute` su una `DispatchQueue` seriale propria, `com.unveil.engine-manager` con QoS `.userInitiated`, e le espone come `async` con `withCheckedContinuation`;
- `openPhoto` esegue `library.import` e poi `library.select`, con le forme documentate in T4;
- il callback C è una funzione `@convention(c)` che riceve `Unmanaged<FrameSink>` come `ctx` e chiama `receive`;
- `deinit` chiama `uv_session_free`.

- [ ] **Step 1: Test**

```swift
//
//  EngineErrorTests.swift
//  Unveil
//

import Testing
import UnveilEngine
@testable import Unveil

struct EngineErrorTests {

    @Test
    func everyStatusMapsToItsCase() {
        #expect(EngineError(status: UV_ERR_INVALID_ARGUMENT.rawValue, message: "m") == .invalidArgument("m"))
        #expect(EngineError(status: UV_ERR_UNKNOWN_COMMAND.rawValue, message: "m") == .unknownCommand("m"))
        #expect(EngineError(status: UV_ERR_ENGINE.rawValue, message: "m") == .engine("m"))
        #expect(EngineError(status: UV_ERR_PANIC.rawValue, message: "m") == .panic("m"))
        #expect(EngineError(status: UV_ERR_SUSPENDED.rawValue, message: "m") == .suspended("m"))
        #expect(EngineError(status: UV_ERR_IO.rawValue, message: "m") == .io("m"))
        #expect(EngineError(status: UV_ERR_TIMEOUT.rawValue, message: "m") == .timeout("m"))
    }

    @Test
    func anUnknownCodeNeverCrashes() {
        #expect(EngineError(status: -99, message: "m") == .unknown(code: -99, message: "m"))
    }
}
```

```swift
//
//  FrameSinkTests.swift
//  Unveil
//

import Metal
import Testing
@testable import Unveil

struct FrameSinkTests {

    let device = MTLCreateSystemDefaultDevice()!

    @Test
    func aFrameIsPublishedWithAlignedRows() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 200, count: 10 * 3 * 4)

        pixels.withUnsafeBytes {
            sink.receive(rgba: $0.baseAddress!, width: 10, height: 3, stride: 40, generation: 1, isDraft: false)
        }

        let frame = sink.latest()
        #expect(frame?.generation == 1)
        #expect((frame?.bytesPerRow ?? 0) >= 40)
        #expect((frame?.bytesPerRow ?? 0) % device.minimumLinearTextureAlignment(for: .rgba8Unorm) == 0)
    }

    @Test
    func anOlderGenerationIsIgnored() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 1, count: 4 * 4 * 4)

        pixels.withUnsafeBytes { raw in
            sink.receive(rgba: raw.baseAddress!, width: 4, height: 4, stride: 16, generation: 5, isDraft: false)
            sink.receive(rgba: raw.baseAddress!, width: 4, height: 4, stride: 16, generation: 3, isDraft: true)
        }

        #expect(sink.latest()?.generation == 5)
    }

    @Test
    func consecutiveFramesAlternateBuffers() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 1, count: 4 * 4 * 4)

        let first = pixels.withUnsafeBytes { raw in
            sink.receive(rgba: raw.baseAddress!, width: 4, height: 4, stride: 16, generation: 1, isDraft: false)
            return sink.latest()?.bufferIndex
        }
        let second = pixels.withUnsafeBytes { raw in
            sink.receive(rgba: raw.baseAddress!, width: 4, height: 4, stride: 16, generation: 2, isDraft: false)
            return sink.latest()?.bufferIndex
        }

        #expect(first != second)
    }

    @Test
    func anOversizedFrameIsDropped() {
        let sink   = FrameSink(device: device, maxPixels: 8)
        let pixels = [UInt8](repeating: 1, count: 16 * 16 * 4)

        pixels.withUnsafeBytes {
            sink.receive(rgba: $0.baseAddress!, width: 16, height: 16, stride: 64, generation: 1, isDraft: false)
        }

        #expect(sink.latest() == nil)
    }
}
```

```swift
//
//  EngineManagerTests.swift
//  Unveil
//

import Testing
import UIKit
@testable import Unveil

struct EngineManagerTests {

    /// A PNG the engine can import: the simulator has no RAW samples.
    func fixtureURL() throws -> URL {
        let url      = FileManager.default.temporaryDirectory.appending(path: "fixture-\(UUID()).png")
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 96, height: 64))
        let data     = renderer.pngData { context in
            UIColor.systemTeal.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 96, height: 64))
        }
        try data.write(to: url)
        return url
    }

    @Test
    func openSetAndPreviewDeliversAFrame() async throws {
        let manager = try EngineManager(maxPixels: 512)
        _ = try await manager.openPhoto(at: fixtureURL())
        try await manager.set(.exposure, to: 0.5)

        let generation = try manager.requestPreview(maxPixels: 512, draft: false)

        try await waitUntil(seconds: 30) { manager.frames.latest()?.generation == generation }
    }

    @Test
    func anUnknownFileIsAnErrorNotACrash() async throws {
        let manager = try EngineManager(maxPixels: 256)
        let url     = FileManager.default.temporaryDirectory.appending(path: "broken-\(UUID()).ARW")
        try Data("not a raw".utf8).write(to: url)

        await #expect(throws: EngineError.self) {
            _ = try await manager.openPhoto(at: url)
        }
    }

    @Test
    func previewWhileSuspendedIsRefused() async throws {
        let manager = try EngineManager(maxPixels: 256)
        _ = try await manager.openPhoto(at: fixtureURL())
        try manager.suspend()

        #expect(throws: EngineError.self) {
            _ = try manager.requestPreview(maxPixels: 256, draft: false)
        }
        try manager.resume()
    }
}
```

`waitUntil(seconds:_:)` va in `UnveilTests/Support/WaitUntil.swift`: interroga la condizione ogni 20 ms con `Task.sleep` e lancia un errore alla scadenza. `EngineManager.init(maxPixels:)` lancia `EngineError` se `uv_session_new` restituisce `NULL`.

- [ ] **Step 2: Eseguire `scripts/test-app.sh`** → `FAIL`, perché i tipi non esistono ancora. **Step 3: Implementare.** **Step 4:** `scripts/test-app.sh` → `PASS`.

  Se sul simulatore la GPU non è disponibile, il motore ricade sul processore: il test deve comunque passare, e il report lo annota.

- [ ] **Step 5: Il coordinatore fa il commit** ("Add the single engine entry point and the shared frame buffers")

---

### Task 10: Import, budget di memoria e ciclo di vita (Swift)

**Partenza proposta:** Sonnet 5.5 high. **Milestone:** M2.

**Files:**
- Create: `Unveil/Core/Protocols/PhotoImporting.swift`, `Unveil/Core/Import/PhotoImporter.swift`, `Unveil/Core/Errors/PhotoImportError.swift`
- Create: `Unveil/Core/Protocols/AvailableMemoryReading.swift`, `Unveil/Core/Memory/MemoryBudgetMonitor.swift`, `Unveil/Core/Memory/ProcessAvailableMemory.swift`
- Create: `Unveil/Core/Lifecycle/EngineLifecycleObserver.swift`
- Test: `UnveilTests/PhotoImporterTests.swift`, `UnveilTests/MemoryBudgetMonitorTests.swift`, `UnveilTests/EngineLifecycleObserverTests.swift`

**Interfaces:**
- Consumes: `EngineDriving` (T9).
- Produces:

```swift
protocol PhotoImporting: Sendable {
    /// Copies a user-picked file into Application Support/Imports and returns the copy.
    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> URL
}

enum PhotoImportError: Error, Equatable {
    case accessDenied(URL)
    case copyFailed(String)
}

protocol AvailableMemoryReading: Sendable {
    func availableBytes() -> UInt64    // os_proc_available_memory() in the app
}

final class MemoryBudgetMonitor<Engine: EngineDriving, Memory: AvailableMemoryReading> {
    init(engine: Engine, memory: Memory)
    /// One third of the memory still available, at least 256 MiB.
    static func budget(forAvailable bytes: UInt64) -> UInt64
    func start()             // applies the budget and listens to didReceiveMemoryWarningNotification
    func handleMemoryWarning()
}

final class EngineLifecycleObserver<Engine: EngineDriving> {
    init(engine: Engine, notificationCenter: NotificationCenter = .default)
    func start()   // willResignActive → suspend, didBecomeActive → resume
}
```

- **Copia:** `PhotoImporter` apre l'URL con `startAccessingSecurityScopedResource()` e lo chiude sempre con `defer`. Copia il file in `Application Support/Imports/<UUID>-<nome>`.
- **Budget:** `MemoryBudgetMonitor` lo applica all'avvio, poi a ogni avviso di memoria lo ricalcola sulla memoria disponibile in quel momento e lo riduce di metà. Esempio: disponibili 3 GiB → budget 1 GiB. Dopo un avviso, con 1,2 GiB disponibili, il budget è 1,2 / 3 / 2 = 205 MiB, che diventa 256 MiB perché è il minimo.

- [ ] **Step 1: Test**

```swift
//
//  MemoryBudgetMonitorTests.swift
//  Unveil
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
}
```

```swift
//
//  EngineLifecycleObserverTests.swift
//  Unveil
//

import Testing
import UIKit
@testable import Unveil

struct EngineLifecycleObserverTests {

    @Test
    func resigningActiveSuspendsAndBecomingActiveResumes() {
        let center   = NotificationCenter()
        let engine   = FakeEngine()
        let observer = EngineLifecycleObserver(engine: engine, notificationCenter: center)
        observer.start()

        center.post(name: UIApplication.willResignActiveNotification, object: nil)
        center.post(name: UIApplication.didBecomeActiveNotification, object: nil)

        #expect(engine.lifecycle == ["suspend", "resume"])
    }
}
```

```swift
//
//  PhotoImporterTests.swift
//  Unveil
//

import Foundation
import Testing
@testable import Unveil

struct PhotoImporterTests {

    @Test
    func aPickedFileIsCopiedIntoImports() throws {
        let source = FileManager.default.temporaryDirectory.appending(path: "picked-\(UUID()).ARW")
        try Data([1, 2, 3]).write(to: source)

        let copy = try PhotoImporter().importCopy(of: source)

        #expect(copy.path().contains("/Imports/"))
        #expect(try Data(contentsOf: copy) == Data([1, 2, 3]))
    }

    @Test
    func aMissingFileIsACopyFailure() {
        let missing = URL(filePath: "/nonexistent/\(UUID()).ARW")
        #expect(throws: PhotoImportError.self) {
            _ = try PhotoImporter().importCopy(of: missing)
        }
    }
}
```

`FakeEngine` (`UnveilTests/Support/FakeEngine.swift`) implementa `EngineDriving`:
- registra `budgets: [UInt64]` e `lifecycle: [String]` sotto `Mutex`;
- `frames` restituisce un `FrameSink` creato con il dispositivo di sistema;
- gli altri metodi registrano la chiamata e non fanno altro.

`FixedMemory(bytes:)` implementa `AvailableMemoryReading` restituendo un valore fisso.

- [ ] **Step 2–4: `FAIL` → implementare → `scripts/test-app.sh` `PASS`.**

- [ ] **Step 5: Il coordinatore fa il commit** ("Copy picked photos into the app and track memory and lifecycle for the engine")

---

### Task 11: CanvasView Metal

**Partenza proposta:** Opus 5.5 high. **Milestone:** M2.

**Files:**
- Create: `Unveil/Views/Editor/CanvasView.swift`, `Unveil/Views/Editor/CanvasShaders.metal`, `Unveil/Models/Editor/CanvasTransform.swift`
- Test: `UnveilTests/CanvasTransformTests.swift`

**Interfaces:**
- Consumes: `FrameSink`, `ReadyFrame` (T9).
- Produces:

```swift
/// CanvasTransform maps the image into the view: aspect fit at scale 1, then zoom and pan.
@frozen
struct CanvasTransform: Equatable, Sendable {

    var zoom  : Double = 1      // 1 = aspect fit; clamped to 1...16
    var offset: SIMD2<Double> = .zero   // in view points

    /// Normalized device coordinates of the image quad's corners for a view and image size.
    func quad(viewSize: CGSize, imageSize: CGSize) -> (min: SIMD2<Float>, max: SIMD2<Float>)
    mutating func pinch(by factor: Double, around point: CGPoint, viewSize: CGSize)
    mutating func pan(by translation: CGPoint)
    mutating func reset()
}

final class CanvasView: UIView {
    init(frames: FrameSink, device: some MTLDevice)
    override class var layerClass: AnyClass { CAMetalLayer.self }
    func setNeedsRedraw()     // marks dirty; one draw on the next CADisplayLink tick, then idle
}
```

- **Disegno:** `CanvasView` crea una texture **senza copia** sul buffer pubblicato, con `buffer.makeTexture(descriptor:offset:0, bytesPerRow:)` e formato `.rgba8Unorm`. La disegna come quad con un vertex shader e un fragment shader minimi. Il drawable è `.bgra8Unorm`, con `colorspace` sRGB: i pixel del motore sono già codificati per il display.
- **Ridisegno:** un `CADisplayLink` attivo **solo** quando c'è qualcosa da disegnare. Si mette in pausa dopo il disegno.
- **Gesti:** pinch e pan aggiornano `CanvasTransform` e chiamano `setNeedsRedraw()`. Un doppio tocco esegue `reset()`.
- **Nuovi frame:** `frames.onFrame` chiama `setNeedsRedraw()`, con un salto su main a coalescenza: un solo salto in attesa alla volta.

- [ ] **Step 1: Test**

```swift
//
//  CanvasTransformTests.swift
//  Unveil
//

import CoreGraphics
import Testing
@testable import Unveil

struct CanvasTransformTests {

    @Test
    func aWideImageFitsTheWidth() {
        let quad = CanvasTransform().quad(viewSize: CGSize(width: 1000, height: 1000), imageSize: CGSize(width: 2000, height: 1000))
        #expect(quad.min.x == -1 && quad.max.x == 1)
        #expect(abs(quad.min.y + 0.5) < 1e-6 && abs(quad.max.y - 0.5) < 1e-6)
    }

    @Test
    func zoomIsClamped() {
        var t = CanvasTransform()
        t.pinch(by: 100, around: .zero, viewSize: CGSize(width: 100, height: 100))
        #expect(t.zoom == 16)
        t.pinch(by: 0.001, around: .zero, viewSize: CGSize(width: 100, height: 100))
        #expect(t.zoom == 1)
    }

    @Test
    func pinchKeepsThePointUnderTheFingers() {
        var t      = CanvasTransform()
        let view   = CGSize(width: 1000, height: 1000)
        let image  = CGSize(width: 1000, height: 1000)
        let finger = CGPoint(x: 750, y: 500)
        let before = t.imagePoint(at: finger, viewSize: view, imageSize: image)

        t.pinch(by: 2, around: finger, viewSize: view)

        let after = t.imagePoint(at: finger, viewSize: view, imageSize: image)
        #expect(abs(before.x - after.x) < 0.5 && abs(before.y - after.y) < 0.5)
    }

    @Test
    func resetReturnsToFit() {
        var t = CanvasTransform()
        t.pinch(by: 3, around: .zero, viewSize: CGSize(width: 100, height: 100))
        t.pan(by: CGPoint(x: 40, y: 10))
        t.reset()
        #expect(t == CanvasTransform())
    }
}
```

Aggiungere a `CanvasTransform` anche `func imagePoint(at viewPoint: CGPoint, viewSize: CGSize, imageSize: CGSize) -> CGPoint`, che è l'inverso di `quad`.

- [ ] **Step 2–4: `FAIL` → implementare → `scripts/test-app.sh` `PASS`.** Poi il verifier controlla la vista con un'**ispezione visiva** sul simulatore, con un frame di prova (il gradiente) iniettato nel `FrameSink` da un comando di debug di T12. Se T12 non è ancora pronto, il controllo visivo slitta a T12 e il report lo annota.

- [ ] **Step 5: Il coordinatore fa il commit** ("Draw engine frames with a Metal canvas reading the shared buffers")

---

### Task 12: Editor: EditorViewModel, EditorViewController, AdjustmentPanel

**Partenza proposta:** Sonnet 5.5 high. **Milestone:** M2.

**Files:**
- Create: `Unveil/Core/Engine/EditorViewModel.swift`, `Unveil/Views/Editor/EditorViewController.swift`, `Unveil/Components/Adjustments/AdjustmentPanel.swift`, `Unveil/Components/Adjustments/AdjustmentSlider.swift`
- Create: `Unveil/Views/Diagnostics/DiagnosticsMenu.swift` (menu di debug, compilato solo in `DEBUG`)
- Modify: `Unveil/App/SceneDelegate.swift`, `Unveil/App/AppDelegate.swift` (composizione dei servizi)
- Test: `UnveilTests/EditorViewModelTests.swift`

**Interfaces:**
- Consumes: T9, T10, T11.
- Produces:

```swift
@Observable
@MainActor
final class EditorViewModel<Engine: EngineDriving, Importer: PhotoImporting> {

    private(set) var values      : [DevelopAdjustmentKind: Double]   // defaults from the kind
    private(set) var openPhoto   : PhotoID?
    private(set) var errorMessage: String?

    init(engine: Engine, importer: Importer, previewPixels: Int)
    func open(pickedURL: URL) async
    func beginDrag(_ kind: DevelopAdjustmentKind)
    func drag(_ kind: DevelopAdjustmentKind, to value: Double) async     // set + draft preview
    func endDrag(_ kind: DevelopAdjustmentKind) async                    // full preview
    func dismissError()
}
```

- **Durante il trascinamento** ogni valore produce `set` e poi `requestPreview(draft: true)`. Al rilascio parte `requestPreview(draft: false)`.
- **Valori:** vengono sempre limitati a `kind.range`.
- **Errori:** un `EngineError` o un `PhotoImportError` finiscono in `errorMessage`, mai in un crash. La vista li mostra in un `UIAlertController`.
- **Interfaccia:** `EditorViewController<Engine, Importer>` ha il `CanvasView` a schermo intero, un pulsante "Apri" (un `UIDocumentPickerViewController` per `UTType.rawImage` e `UTType.image`) e l'`AdjustmentPanel` SwiftUI in un `UIHostingController` laterale, con i 10 slider. Liquid Glass e il design definitivo **non** sono in questo ticket.
- **`DiagnosticsMenu` (solo DEBUG):** tre voci che restano vuote fino a T14 e T15: "Inietta frame di prova" (gradiente nel `FrameSink`), "Esporta immagini di riferimento", "Prova di durata 10 min".

- [ ] **Step 1: Test**

```swift
//
//  EditorViewModelTests.swift
//  Unveil
//

import Foundation
import Testing
@testable import Unveil

@MainActor
struct EditorViewModelTests {

    @Test
    func dragSendsValueThenDraftPreviewAndReleaseSendsFull() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        model.beginDrag(.exposure)
        await model.drag(.exposure, to: 0.7)
        await model.endDrag(.exposure)

        #expect(engine.calls == ["set light.exposure 0.7", "preview 1024 draft", "preview 1024 full"])
    }

    @Test
    func valuesAreClampedToTheControlRange() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.drag(.exposure, to: 99)

        #expect(model.values[.exposure] == DevelopAdjustmentKind.exposure.range.upperBound)
    }

    @Test
    func anEngineErrorBecomesAMessageNotACrash() async {
        let engine = FakeEngine()
        engine.failNextOpen = .engine("unsupported camera")
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.open(pickedURL: URL(filePath: "/tmp/x.ARW"))

        #expect(model.errorMessage?.contains("unsupported camera") == true)
        #expect(model.openPhoto == nil)
    }
}
```

Estendere `FakeEngine`:
- `calls: [String]` (formato `"set <id> <valore>"`, `"preview <px> draft|full"`, `"open <path>"`);
- `failNextOpen: EngineError?`.

`FakeImporter` restituisce l'URL che riceve.

- [ ] **Step 2–4: `FAIL` → implementare → `scripts/test-app.sh` `PASS`.** Poi si avvia l'app sul simulatore e si verifica:
  - apertura di un JPEG o PNG dall'app File del simulatore;
  - lo slider dell'esposizione cambia l'immagine;
  - "Inietta frame di prova" mostra il gradiente.

  Il verifier allega due screenshot al report.

- [ ] **Step 5: Il coordinatore fa il commit** ("Add the minimal editor: canvas, ten sliders and file opening")

**🛑 Fine M2.** Il coordinatore si ferma con il riepilogo, il `git log m2-app` e gli screenshot del simulatore. L'autore fa il push.

---

# M3 — Baseline sul dispositivo

### Task 13: 👤 Prima esecuzione sull'iPad M2

**Partenza proposta:** Sonnet 5.5 high, con l'autore presente. **Milestone:** M3.

**Files:**
- Create: `Docs/Baseline/device-run.md`

- [ ] **Step 1 (👤):** collegare l'iPad M2, attivare la modalità sviluppatore e accettare il computer come attendibile.

- [ ] **Step 2: Installare e avviare**

```bash
xcrun devicectl list devices
DEVICE=<identifier dell'iPad>
xcodebuild -project Unveil.xcodeproj -scheme Unveil -configuration Release \
    -destination "id=$DEVICE" -derivedDataPath DerivedData-device build
xcrun devicectl device install app --device "$DEVICE" DerivedData-device/Build/Products/Release-iphoneos/Unveil.app
xcrun devicectl device process launch --device "$DEVICE" <bundle id>
```

- [ ] **Step 3 (👤): Lista di controllo da fare a mano.** Ogni voce va annotata OK/KO in `device-run.md`:
  1. Aprire un RAW da circa 24 MP dall'app File → l'anteprima compare.
  2. Muovere ognuno dei 10 slider → l'immagine cambia mentre trascini e si rifinisce al rilascio.
  3. Pinch, pan e doppio tocco sul canvas.
  4. Ripetere 1 e 2 con un RAW da circa 48 MP.
  5. **Background durante un render:** muovere uno slider, passare subito a un'altra app, tornare dopo 5 s → gli slider rispondono ancora con la stessa velocità (la GPU è ancora in uso; il log del backend lo conferma in T15).
  6. Aprire un file non immagine rinominato `.ARW` → compare un messaggio d'errore e l'app resta aperta.

  I RAW per i punti 1 e 4 sono quelli di T14 Step 1. Se T14 non è ancora fatto, si scaricano prima quei due file, con conferma.

- [ ] **Step 4: Il coordinatore fa il commit** di `device-run.md` ("Record the first run on iPad M2").

---

### Task 14: Immagini di riferimento ed equivalenza

**Partenza proposta:** Sonnet 5.5 high. **Milestone:** M3.

**Files:**
- Create: `Engine/ffi/tests/golden.rs`, `Engine/ffi/examples/compare_golden.rs`, `Engine/ffi/src/delta_e.rs`
- Create: `Unveil/Core/Diagnostics/GoldenExporter.swift`; Modify: `Unveil/Views/Diagnostics/DiagnosticsMenu.swift`
- Create: `Docs/Baseline/raw-set.md`, `Docs/Baseline/equivalence.md`

**Interfaces:**
- **Combinazioni di regolazioni**, identiche su Mac e iPad. Definite una volta in `Docs/Baseline/raw-set.md` e copiate in Rust e in Swift:
  - `neutral`: nessuna regolazione;
  - `exposure`: `light.exposure` = +1.0;
  - `contrast`: `light.contrast` = +60;
  - `temperature`: `wb.temp` = 4000;
  - `tones`: `light.shadows` = +50 e `light.highlights` = −50.
- **Render:** completo (non draft), `max_pixels = 2048`.
- **Nomi dei file:** `<raw-basename>__<preset>.png`.
- `delta_e::ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64` e `delta_e::srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3]` (D65).

- [ ] **Step 1 (👤): Scegliere e scaricare i RAW.** Il worker propone da https://raw.pixls.us un elenco di 5 file (fotocamera, formato, MP, licenza, dimensione):
  - almeno un X-Trans (RAF);
  - almeno due intorno ai 24 MP;
  - almeno due tra 45 e 48 MP;
  - formati ARW, NEF, CR3, RAF, DNG.

  **Il coordinatore chiede conferma all'autore prima di ogni download.** I file vanno in `Baseline/raw/`, che è ignorata da git. `raw-set.md` registra URL, licenza e SHA-256 di ciascuno.

- [ ] **Step 2: Test di ΔE2000 con i valori di Sharma (2005)**, in `delta_e.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharma_reference_pairs() {
        let pairs = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ];
        for (a, b, expected) in pairs {
            let d = ciede2000(a, b);
            assert!((d - expected).abs() < 1e-4, "{a:?} vs {b:?}: {d} != {expected}");
        }
    }

    #[test]
    fn identical_colors_have_zero_distance() {
        let lab = srgb8_to_lab([120, 30, 200]);
        assert_eq!(ciede2000(lab, lab), 0.0);
    }

    #[test]
    fn srgb_white_is_l100() {
        let lab = srgb8_to_lab([255, 255, 255]);
        assert!((lab[0] - 100.0).abs() < 0.01 && lab[1].abs() < 0.01 && lab[2].abs() < 0.01, "{lab:?}");
    }
}
```

  `delta_e` è `pub mod`, non esportato in C. `cargo test -p unveil-ffi delta_e` → prima `FAIL`, poi `PASS`.

- [ ] **Step 3: `tests/golden.rs`.** Se `UNVEIL_RAW_DIR` non è impostata, stampa `skipped: UNVEIL_RAW_DIR not set` e passa. Altrimenti, per ogni RAW e ogni combinazione, esegue import, select, `develop.set` e `uv_request_preview(2048, false)`. Il PNG si scrive con il crate `png` in `Baseline/golden/mac/`.

```bash
UNVEIL_RAW_DIR=Baseline/raw cargo test -p unveil-ffi --release --test golden -- --nocapture
```

  Expected: 25 PNG.

- [ ] **Step 4: `GoldenExporter` (Swift, DEBUG).** Prima si copiano i RAW nel contenitore dell'app:

```bash
xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer \
    --domain-identifier <bundle id> --source Baseline/raw --destination Documents/raw
```

Poi la stessa sequenza di Step 3 tramite `EngineManager`, sui file in `Documents/raw`. Scrive i PNG in `Documents/golden/` con `CGImageDestination` a partire dal buffer del `FrameSink`. Poi:

```bash
xcrun devicectl device copy from --device "$DEVICE" --domain-type appDataContainer \
    --domain-identifier <bundle id> --source Documents/golden --destination Baseline/golden/ipad
```

- [ ] **Step 5: `examples/compare_golden.rs`.** Per ogni coppia `mac/X.png` e `ipad/X.png` calcola media e 99° percentile del ΔE2000. Esce con codice 1 se una coppia supera **media 0,5** o **p99 2,0**, oppure se le dimensioni differiscono.

```bash
cargo run -p unveil-ffi --release --example compare_golden -- Baseline/golden/mac Baseline/golden/ipad | tee Docs/Baseline/equivalence.md
```

  Expected: una tabella con 25 righe ed esito. Se una coppia fallisce, il coordinatore si ferma: è un dato della baseline, non un bug da correggere di nascosto.

- [ ] **Step 6: Il coordinatore fa il commit** ("Compare iPad renders with the Mac reference")

---

### Task 15: Misure

**Partenza proposta:** Opus 5.5 medium. **Milestone:** M3.

**Files:**
- Create: `Unveil/Core/Diagnostics/Signposts.swift`, `Unveil/Core/Diagnostics/StressSweep.swift`, `Unveil/Core/Diagnostics/FootprintSampler.swift`
- Modify: `EditorViewModel.swift`, `EngineManager.swift`, `CanvasView.swift` (solo signpost), `DiagnosticsMenu.swift`
- Create: `Docs/Baseline/measurements.md`

**Interfaces:**
- **`Signposts`:** un `OSSignposter` (subsystem `com.unveil`, category `Engine`) con questi intervalli:
  - `OpenToFirstFrame`: da `open(pickedURL:)` al primo frame disegnato;
  - `SliderToFrame`: dalla chiamata a `requestPreview` al disegno del frame con la stessa generazione, con l'argomento `draft|full`.
- **`FootprintSampler`:** ogni 2 s legge `phys_footprint` (`task_info` con `TASK_VM_INFO`), `os_proc_available_memory()` e `ProcessInfo.thermalState`, e li scrive in CSV in `Documents/measurements/<data>.csv`.
- **`StressSweep`:** per 10 minuti fa oscillare `light.exposure` tra −2 e +2:
  - a ogni tick di un `CADisplayLink` a 60 Hz parte un drag in draft;
  - ogni secondo parte un rilascio con render completo;
  - nel frattempo è attivo `FootprintSampler`.
- **Backend:** all'avvio si registra con `os.Logger` se il motore usa la GPU o il processore. È un comando del motore se esiste, altrimenti il campo `gpu` dell'ispezione: il worker lo cerca in `Engine/crates/engine` e lo documenta.

- [ ] **Step 1: Implementare e verificare sul simulatore** che i signpost compaiano:

```bash
SIM=<id del simulatore iPad usato da scripts/test-app.sh>
xcrun xctrace record --template 'Logging' --device "$SIM" --time-limit 20s --output /tmp/sim.trace \
    --launch -- DerivedData/Build/Products/Debug-iphonesimulator/Unveil.app
```

- [ ] **Step 2 (👤 iPad collegato): Registrare sull'M2**, con il RAW da 48 MP già aperto:

```bash
mkdir -p Baseline/traces
xcrun xctrace record --device "$DEVICE" --template 'Time Profiler' --attach Unveil --time-limit 60s --output Baseline/traces/time-48mp.trace
xcrun xctrace record --device "$DEVICE" --template 'Metal System Trace' --attach Unveil --time-limit 30s --output Baseline/traces/metal-48mp.trace
```

  Durante la registrazione si usa "Prova di durata" oppure si muovono gli slider a mano. Infine si avvia la prova di durata completa da 10 minuti e si copia il CSV con `devicectl device copy from`.

- [ ] **Step 3: Confronto A/B `target-cpu`:**

```bash
scripts/build-xcframework.sh --cpu apple-m1
```

  Si ricompila l'app e si ripetono solo le misure `SliderToFrame` (30 trascinamenti, poi mediana e p95). Al termine si ricostruisce con `scripts/build-xcframework.sh` senza opzioni.

- [ ] **Step 4: Scrivere `Docs/Baseline/measurements.md`**, con le misure della spec §6.2 per 24 e 48 MP:
  - mediana e p95;
  - picco di `phys_footprint`;
  - minimo di memoria disponibile;
  - transizioni termiche;
  - backend effettivo;
  - A/B `target-cpu`.

  Ogni numero porta il comando o la traccia da cui viene.

- [ ] **Step 5: Il coordinatore fa il commit** ("Measure open, slider and memory behaviour on iPad M2")

---

### Task 16: Rapporto di profilazione e mappa delle strutture dati

**Partenza proposta:** Opus 5.5 high. **Milestone:** M3.

**Files:**
- Create: `Docs/Baseline/profiling.md`, `Docs/Baseline/data-structures.md`

**Interfaces:**
- Consumes: le tracce di T15, il codice in `Engine/crates/`.

- [ ] **Step 1: Esportare i dati delle tracce**

```bash
xcrun xctrace export --input Baseline/traces/time-48mp.trace --toc > /tmp/toc.xml
xcrun xctrace export --input Baseline/traces/time-48mp.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' > /tmp/time-profile.xml
```

  Se i simboli Rust non sono risolti, si verifica che `Engine/Cargo.toml` abbia `[profile.release] debug = "line-tables-only"` (upstream lo ha) e che esista il dSYM dell'app. Le soluzioni provate vanno annotate.

- [ ] **Step 2: `profiling.md`** contiene:
  - le 20 funzioni con più tempo esclusivo e inclusivo;
  - la ripartizione CPU/GPU per `SliderToFrame`, ricavata dalla Metal System Trace (tempo GPU per frame rispetto al tempo totale);
  - ogni copia di memoria individuata (readback, copia in `FrameSink`, altre) con il suo costo;
  - i 5 obiettivi più promettenti per il sottoprogetto 2, ordinati per tempo recuperabile stimato.

- [ ] **Step 3: `data-structures.md`.** Per ogni struttura o buffer sul percorso critico (mosaico RAW, immagini `Rgb32f`, `Plane`, cache di stadio, buffer wgpu, `Rgba8` di uscita, `MTLBuffer` del `FrameSink`) indica:
  - tipo e file:riga;
  - layout (planare o interlacciato);
  - tipo degli elementi;
  - byte per pixel;
  - dimensione a 24 e a 48 MP;
  - quando viene allocata (una volta, per foto, per render);
  - quota di tempo dal profilo.

  Ogni stima è segnata come calcolata o misurata.

- [ ] **Step 4: Il coordinatore fa il commit e crea il tag**

```bash
git commit -F <messaggio>   # "Profile the baseline and map the hot data structures"
git tag baseline-v0
```

**🛑 Fine M3 e della v0.** Il coordinatore consegna all'autore:
- riepilogo;
- `Docs/Baseline/` completo;
- l'esito dei criteri di accettazione della spec §3, uno per uno;
- la proposta di avviare il brainstorming del sottoprogetto 2 a partire da `profiling.md` e `data-structures.md`.
