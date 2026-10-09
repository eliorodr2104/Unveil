# v0 — Motore LightCraft su iPad (baseline)

**Data:** 9 ottobre 2026. **Stato:** bozza in revisione. **Sottoprogetto:** 1 di 3.
**Nome del prodotto:** Unveil (deciso il 9 ottobre 2026). Prefisso della C ABI: `uv_`.

## 1. Contesto e decisioni prese

- Il prodotto è un editor RAW per iPad, **personale**, con un marchio proprio (non LightCraft, non Forte). È pensato per **fotografi professionisti che lavorano in RAW**.
- L'interfaccia futura è radicale: tutto ruota attorno al canvas e alla Pencil, con controlli Liquid Glass flottanti e contestuali. La precisione deve essere sempre raggiungibile. Questo documento **non** la tratta.
- La strategia sul motore:
  1. **v0 (questo documento):** il motore upstream [storytold/lightcraft](https://github.com/storytold/lightcraft) (Apache-2.0), invariato e senza AI, gira su iPad. Serve da **baseline**.
  2. **Motore Apple-native:** il motore incluso in `Engine/` evolve in Rust, con `objc2-metal` al posto di wgpu, Metal 4 dove conviene, memoria unificata, NEON e layout dei dati ottimizzato. Ogni modifica va misurata contro la v0.
  3. **Prova di Core Image RAW 9,** come decoder alternativo per singola foto.
- Il linguaggio resta **Rust** per il motore e Swift per l'app. Swift non sostituisce il codice di calcolo: i passaggi critici vanno su Metal o NEON.
- **Versione minima: iPadOS 26.** Dispositivi: iPad con M1 e successivi. Dispositivo di prova: l'iPad M2 dell'autore. L'M1 non è testato; i valori di memoria sono un'approssimazione ragionevole, le prestazioni restano un'ipotesi.
- **Commit upstream fissato:** `c435d143de921e8dc724245065500bc191f91102`. È il commit verificato dall'analisi statica del 9 ottobre 2026.

## 2. Obiettivo e perimetro

**Obiettivo.** Il motore completo di LightCraft, compilato senza opzioni AI, gira sull'iPad M2 dentro un'app UIKit minimale. L'app mostra una foto e permette di regolarne gli attributi di base. Tutte le funzioni di sviluppo restano nel motore e si possono raggiungere tramite comandi, anche se non tutte hanno ancora un controllo nell'interfaccia.

**Incluso:**
- Apertura di un RAW dall'app File. Il file viene copiato nel contenitore dell'app.
- Canvas: la foto adattata allo schermo, con zoom tramite pinch.
- Pannello slider con: esposizione, contrasto, luci, ombre, bianchi, neri, temperatura, tinta, vividezza, saturazione.
- Anteprima veloce (draft) durante il trascinamento e render completo al rilascio.
- Gestione corretta di background, avvisi di memoria e panic.
- Strumenti di misura e di confronto (sezione 6).

**Escluso, rimandato ai sottoprogetti successivi:**
- salvataggio e riapertura delle modifiche, sidecar XMP, export;
- vista 1:1 a piena risoluzione;
- Pencil, gesti, Liquid Glass, libreria, AI;
- qualsiasi ottimizzazione del motore, eliminazione del readback del motore (il canvas Metal invece è incluso), HDR/EDR.

## 3. Criteri di accettazione

1. `Engine/ffi` compila per `aarch64-apple-ios`, `aarch64-apple-ios-sim` e `aarch64-apple-darwin`. Lo script produce l'XCFramework.
2. Sull'iPad M2 si aprono un RAW da 24 MP e uno da 48 MP, e gli slider aggiornano il canvas. Niente crash, niente chiusure per memoria (jetsam).
3. Test di durata: 10 minuti di slider automatici sul 48 MP, senza jetsam. Stato termico e memoria vengono registrati.
4. Equivalenza con il Mac: stesse foto e stesse regolazioni producono differenze ΔE2000 con **media < 0,5** e **99° percentile < 2**. Sono soglie approvate: ΔE < 1 è invisibile all'occhio. Si possono cambiare solo con una motivazione scritta, sulla base dei primi risultati.
5. `cargo test` sul Mac copre tutti i test del bridge elencati nella sezione 6.5.
6. In `Docs/Baseline/` ci sono: le misure, il rapporto di profilazione e la mappa delle strutture dati calde (sezione 6).

## 4. Architettura

```
 UIKit (main)                    Swift                             Rust (thread del motore)
 ┌─────────────────────┐ azioni ┌──────────────────┐   C ABI    ┌──────────────────────────┐
 │ EditorViewController├───────►│ EditorViewModel  │            │ uv_* → Session (upstream) │
 │ AdjustmentPanel     │        │ (@Observable)    │            │ PreviewScheduler          │
 │ CanvasView (Metal) ◄┼──┐     │       │          │            │ vince l'ultima richiesta  │
 └─────────────────────┘  │     │ EngineManager ───┼──────────►│                           │
                          │     │ (unica porta)   ◄┼───────────┤ callback frame            │
                          │     └───────┬──────────┘            └──────────────────────────┘
                          └─ MTLBuffer condiviso (scritto nella callback, letto dal canvas)
```

**Dove va la v0 e dove va il sottoprogetto 2.** Il canvas legge l'anteprima da un `MTLBuffer` in memoria condivisa (unified memory), senza `UIImage`. Nella v0 il motore upstream fa ancora un readback in RGBA8, e la callback copia **una volta** quel risultato nel buffer condiviso. Nel sottoprogetto 2 il motore scriverà direttamente in memoria condivisa sulla GPU e la copia sparirà. Il lato UI non cambia, quindi il guadagno si misura confrontando con la v0.

### 4.1 C ABI (`Engine/ffi/include/uv.h`, scritto a mano)

Il prefisso `uv_` sta per Unveil.

```c
uint32_t    uv_abi_version(void);
UVSession*  uv_session_new(const char* data_dir, uint64_t mem_budget);
void        uv_session_free(UVSession*);

int32_t     uv_execute(UVSession*, const char* cmd, const char* params_json,
                       char** result_json);          // risultato liberato con uv_string_free
void        uv_string_free(char*);
const char* uv_last_error(void);                     // per thread, come errno

typedef void (*uv_frame_cb)(void* ctx, const uint8_t* rgba, uint32_t w, uint32_t h,
                            uint32_t stride, uint64_t generation, bool draft);
uint64_t    uv_request_preview(UVSession*, uint32_t max_px, bool draft,
                               uv_frame_cb, void* ctx);       // generazione, 0 = errore

int32_t     uv_suspend(UVSession*);
int32_t     uv_resume(UVSession*);
void        uv_set_memory_budget(UVSession*, uint64_t bytes);
```

- `uv_execute` inoltra il comando a `Session::execute(id, &Value)` del motore. È lo stesso sistema di comandi usato dal protocollo di controllo e dal server MCP di upstream. I comandi esatti per aprire una foto e per `develop.set` li ricava il piano da `engine.commands`.
- **Errori tra i linguaggi.** I codici di ritorno sono un `enum UVStatus` definito in `uv.h`: `UV_OK = 0`, poi valori negativi per argomento non valido, comando sconosciuto, errore del motore, panic catturato, sessione sospesa, I/O e timeout. Il messaggio si legge con `uv_last_error`. Lato Swift, `EngineError` (`Core/Errors/`) ha un caso per ogni codice: lo costruisce `EngineManager`, che lo lancia con `throw` insieme al messaggio. Un codice che Swift non conosce diventa `.unknown(code)` e non va mai in crash.

### 4.2 Lato Rust (`Engine/ffi`)

- **Motore incluso nel repository (vendored) dal primo giorno.** `Engine/` contiene una copia del sorgente upstream al commit fissato: `crates/` senza `ui-egui` e `mcp`, `assets/` (servono i profili camera e il font Inter), `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `clippy.toml`. Sono esclusi `apps/`, `xtask/`, `docs/` e quindi i marchi ArtCraft in `docs/brand/`. In `Engine/UPSTREAM.md` si registrano commit, data e cosa è stato escluso. Nella v0 il codice copiato **non si modifica**: cambia solo l'elenco `members` del workspace (`crates/*` meno gli esclusi, più `ffi`). Il tag git `baseline-v0` segna lo stato di riferimento.
- **Feature:** `lightcraft-engine` senza feature, quindi niente `sam`, `denoise` e `rawnind-model`.
- **Lint:** `ffi` è membro del workspace ma non eredita i lint del workspace (`unsafe_code = "deny"`). Il crate dichiara i propri lint e `unsafe` resta confinato al bridge.
- **Panic:** ogni funzione `extern "C"` passa da `panic_guard.rs` (`catch_unwind`). Un panic diventa un codice d'errore, mai un abort.
- **Thread:** il thread `unveil-engine` possiede la `Session` ed esegue i comandi; un secondo thread `unveil-render` esegue `RenderJob::run`, così `uv_execute` aspetta solo il comando (millisecondi), mai un render. Il thread del motore è creato da Rust, con dimensione dello stack esplicita e QoS `USER_INITIATED`. La `Session` vive solo su questo thread. I comandi e le richieste arrivano tramite un canale.
- **PreviewScheduler:** è la versione ridotta del `Renderer` di `ui-egui`. Gestisce un render alla volta. Una richiesta che arriva durante un render sostituisce quella in attesa, e il numero di generazione cresce. Il render in corso non viene interrotto, a meno che il piano non trovi un modo di annullarlo in upstream. Usa i job builder di `Session` (`render_job`, la modalità `draft` durante il trascinamento) e chiama `session.accept(...)` per mettere in cache le sorgenti decodificate.
- **Callback dei frame:** viene chiamata sul thread del motore. Il buffer `rgba` è valido **solo durante la callback**.
- **Memoria:** `uv_set_memory_budget` chiama `memory::set_budget`. All'avvio `memory::set_release_hook` riceve `malloc_zone_pressure_relief`, che esiste anche su iOS.
- **Sospensione e ripresa:** `uv_suspend` smette di accettare nuovi render e aspetta che finisca il lavoro GPU in corso. `uv_resume` chiama `lightcraft_gpu::reset_failures()`. Serve perché il motore marca la GPU come rotta per sempre dopo un errore del dispositivo.
- **Percorsi:** si usano solo i percorsi espliciti sotto `data_dir`, mai quelli predefiniti basati su `$HOME`. Sidecar disattivati. Non vanno chiamati `with_default_face_models` e `with_default_denoise_models`.

### 4.3 Lato Swift (`Unveil/`)

- **`EngineManager`** (`Core/Engine/`) è **l'unica porta verso il motore**. Nessun altro tipo chiama `uv_*`.
  - È l'unica implementazione di `EngineDriving` (`Core/Protocols/`). Possiede l'handle e offre `open(url:)`, `set(_:_:)` e `requestPreview(draft:)`.
  - Le chiamate non bloccano mai, perché ordine e serializzazione li gestisce il thread del motore in Rust.
  - **Sincronizzazione:** si usa `Mutex` (Synchronization) per lo stato condiviso con la callback. Niente actor. `DispatchSemaphore` serve solo come segnale tra thread dedicati, sempre con timeout, e mai sul thread principale.
  - **Frame:** la callback copia il frame, con `RawSpan`/`MutableRawSpan`, in uno di due `MTLBuffer` condivisi preallocati che si alternano (double buffering). Poi pubblica sotto `Mutex` l'indice del buffer pronto e la sua generazione. Il canvas legge l'ultimo buffer pronto al frame successivo. Non ci sono hop su main per ogni frame.
- **`EditorViewModel`** (`Core/Engine/`, `@Observable`, `@MainActor`) tiene lo stato dell'interfaccia: foto aperta, valori degli slider, stato del draft, errore da mostrare. Parla solo con `EngineManager` ed è generico su `Engine: EngineDriving`.
- **`PhotoImporter`** (`Core/Import/`): apre l'URL con permesso di sicurezza (security-scoped) e copia il file in `Application Support/Imports/`.
- **`MemoryBudgetMonitor`** (`Core/Memory/`): calcola il budget su `os_proc_available_memory()` e lo abbassa agli avvisi di memoria.
- **`EngineLifecycleObserver`** (`Core/Lifecycle/`): chiama `uv_suspend` quando l'app perde il primo piano (`willResignActive`) e `uv_resume` quando lo riacquista (`didBecomeActive`).
- **`EditorViewController`** e **`CanvasView`** (`Views/Editor/`):
  - `CanvasView` è una vista con `CAMetalLayer`. Un piccolo shader disegna il buffer condiviso come texture, e pinch e pan sono una trasformazione applicata nello shader. Ridisegna solo quando arriva una nuova generazione o cambia lo zoom, mai in un ciclo continuo.
  - L'`EditorViewController` è generico sul motore: `EditorViewController<Engine: EngineDriving>`, così il dispatch resta statico.
- **`AdjustmentPanel`** (`Components/Adjustments/`): un pannello SwiftUI dentro un `UIHostingController`.
- **Entitlement** (nel file `.entitlements`, non nell'`Info.plist`): `increased-memory-limit` ed `extended-virtual-addressing`. Alzano il limite di memoria senza dire di quanto, quindi il budget resta quello calcolato su `os_proc_available_memory()`. Il piano deve verificare che siano disponibili con il team di firma usato.
- **Background:** iOS non permette alle app in background di usare la GPU per le anteprime, e nessuna chiave del plist o thread dedicato cambia le cose. Per l'export futuro si valuterà `BGContinuedProcessingTask` con la risorsa GPU (iPadOS 26), che richiede un entitlement e un dispositivo supportato, da verificare sull'M2. Fuori perimetro v0.

## 5. Rischi noti e mitigazioni

Dall'analisi statica del commit fissato:

| Rischio | Mitigazione nella v0 |
|---|---|
| GPU marcata rotta dopo un render in background | `uv_suspend`/`uv_resume` con `reset_failures()`; test manuale: uscire dall'app durante un render |
| Panic che attraversa l'FFI → abort | `catch_unwind` su ogni funzione esportata; job su thread propri |
| Budget di memoria fisso a 1,5 GiB su iOS; picco di 2–3 GB a 48 MP con la piena risoluzione | budget da Swift; niente 1:1 né export nella v0; l'anteprima resta ≤ 2560 px (il motore riduce il mosaico) |
| Percorsi `$HOME/.config` e riletture dal percorso originale | copia nel contenitore e percorsi espliciti |
| Readback sempre in RGBA8 verso la CPU | nella v0 una sola copia nel `MTLBuffer` condiviso letto dal canvas Metal; eliminarla è il primo obiettivo del sottoprogetto 2 |
| Render in corso non interrompibile in upstream | le richieste in attesa e i risultati superati vengono scartati; l'annullamento vero è candidato al sottoprogetto 2 |
| Dipendenze non verificate su iOS (`ring`, `ash`, `cc`, build script di `rav1e`) | il primo passo del piano è `cargo tree` e una build per `aarch64-apple-ios` |

## 6. Test, misure e baseline

### 6.1 Immagini di riferimento
- **Sul Mac:** un test Rust in `Engine/ffi/tests/` usa le stesse chiamate `uv_*` e gli stessi comandi JSON e salva le anteprime in PNG.
- **Sull'iPad:** un comando di debug produce gli stessi PNG in Documenti.
- **Confronto:** `Engine/ffi/examples/compare_golden.rs` calcola il ΔE2000 tra le due serie. È in Rust per non aggiungere dipendenze Python, e la formula è verificata con le coppie di riferimento di Sharma (2005).
- **Set di prova:** 3–5 RAW da fotocamere diverse, a 24 e 48 MP. Di default sono campioni a licenza libera da raw.pixls.us, più le foto dell'autore se le fornisce. Ogni RAW viene provato con 4–5 combinazioni: neutra, esposizione, contrasto forte, temperatura, ombre e luci.

### 6.2 Misure (iPad M2, release con informazioni di debug e dSYM)

| Misura | Strumento |
|---|---|
| Tempo alla prima anteprima (24 e 48 MP) | `os_signpost` |
| Latenza slider → frame mostrato (draft e completo) | `os_signpost` più il numero di generazione |
| Picco di `phys_footprint` | Instruments più un log periodico |
| Test di durata di 10 minuti | comando di debug che muove gli slider; registra lo stato termico |
| GPU o CPU e ripieghi | log del backend effettivo |
| `-C target-cpu=apple-m1` contro il default (`apple-a7`) | confronto A/B su latenza e profilo |

Con l'iPad collegato, gli agenti installano l'app e registrano le tracce con `xcrun xctrace`.

### 6.3 Rapporto di profilazione
Registrazioni Time Profiler e Metal System Trace durante l'apertura e il test con gli slider. Contiene le 20 funzioni più costose, la ripartizione CPU/GPU e le copie di memoria individuate.

### 6.4 Mappa delle strutture dati calde
Per ogni struttura o buffer sul percorso critico:
- tipo e layout (planare o interlacciato);
- tipo degli elementi;
- dimensione a 24 e a 48 MP;
- frequenza di allocazione;
- quota di tempo.

È la base delle proposte di ottimizzazione del sottoprogetto 2.

### 6.5 Test del bridge (`cargo test`, Mac)
- versione dell'ABI;
- apertura → comando → anteprima;
- un panic diventa un codice d'errore;
- l'ultima richiesta sostituisce quella in attesa;
- sospensione e ripresa;
- `uv_string_free` e `uv_last_error` non perdono memoria (controllo con `dhat` o equivalente).

## 7. Repository e stile del codice

### 7.1 Struttura

```
Unveil/
├─ Unveil.xcodeproj
├─ Unveil/
│  ├─ App/           AppDelegate, SceneDelegate, composizione dei servizi
│  ├─ Core/          Engine/ Memory/ Lifecycle/ Import/ Diagnostics/
│  │  ├─ Protocols/  un protocollo per file
│  │  ├─ Errors/     un tipo Error per file
│  │  └─ Extensions/ Tipo+Capacità.swift
│  ├─ Models/        per area; enum in Models/<Area>/Enums/
│  ├─ Views/         Editor/ Diagnostics/
│  ├─ Components/    Adjustments/
│  └─ Resources/
├─ UnveilTests/
├─ Engine/           workspace Rust: crates/ (motore upstream incluso), assets/, ffi/ (include/, src/, tests/), UPSTREAM.md
├─ Frameworks/       XCFramework generato, ignorato da git
├─ Config/           UnveilEngine.xcconfig (flag di link generati)
├─ scripts/          build-xcframework.sh, test-app.sh
├─ Docs/             Specs/ Plans/ Baseline/
└─ CODE_STYLE.md  CLAUDE.md  LICENSE  NOTICE  README.md
```

**Licenza:** tutto il repository è sotto **Apache-2.0** (`LICENSE`). Upstream è dual-licensed MIT OR Apache-2.0 (`crates/segment` e `crates/fetch` solo Apache-2.0), quindi lo si usa sotto l'opzione Apache-2.0. Il `NOTICE` alla radice attribuisce LightCraft e riporta il `NOTICE` upstream (Inter OFL 1.1, porting SAM 3). I marchi ArtCraft non vengono copiati. Dal sottoprogetto 2 ogni file del motore modificato porta una nota di modifica, come richiede Apache-2.0 §4(b).

### 7.2 Stile

Il riferimento è il `CODE_STYLE.md` di Cascade. Lo si riprende in `CODE_STYLE.md` con questi adattamenti e aggiunte:

- **UIKit al posto di AppKit;** SwiftUI solo per pannelli e componenti foglia. Con iPadOS 26, `InlineArray` e `Span` si usano senza controlli di versione.
- **Commenti `//` al massimo 2 righe.** Se serve di più, va in un `///` sul simbolo. I `///` restano narrativi, in inglese, nello stile antirez.
- **Orientamento ai protocolli statico:**
  - si usano generici vincolati e `some`, non esistenziali `any`;
  - `any` solo dove serve davvero un tipo eterogeneo, con un commento che lo giustifica;
  - in Rust: generici e `impl Trait`, niente `dyn` sul percorso critico.
- **Prima lo stack, poi l'heap:**
  - valori, `InlineArray` e `Span` / `MutableSpan` / `RawSpan` per accedere ai buffer senza copie né ARC;
  - l'heap si usa solo quando porta un guadagno misurato o quando serve per forza;
  - i buffer di pixel stanno su memoria allocata una volta e riusata, mai allocata per ogni frame.
- **Sul percorso critico vince il basso livello:** `@frozen`, layout compatti e allineati, `Unsafe*` dietro API sicure con il contratto documentato. Ogni scelta di questo tipo si giustifica con una misura.
- **Swift:** 4 spazi, righe da 100 a 120 caratteri, allineamento in colonne e ritmo verticale come nel `CODE_STYLE` di Cascade, un tipo per file.
- **Rust:**
  - `rustfmt` con 4 spazi e `max_width = 110` (in `Engine/ffi/rustfmt.toml`), senza allineamento manuale. Il codice upstream copiato mantiene il proprio `rustfmt.toml` (`max_width = 150`) per non generare differenze nella v0; il motore si riformatta quando lo si modifica nel sottoprogetto 2;
  - file in snake_case, un modulo per responsabilità;
  - ogni blocco `unsafe` ha un commento `// SAFETY:` (al massimo 2 righe) che spiega chi possiede il buffer e per quanto tempo.

## 8. Prerequisiti

- Xcode 27 (installato). Rust 1.97.1 stable, installato via Homebrew: `cargo` è in `/opt/homebrew/opt/rustup/bin`, che non è nel `PATH`, quindi gli script lo aggiungono. I target `aarch64-apple-ios` e `aarch64-apple-ios-sim` sono da aggiungere.
- L'iPad M2 collegato al Mac, con la modalità sviluppatore attiva.
- Un team di firma Apple, anche quello personale.
- RAW di prova (sezione 6.1).

## 9. Decisioni aperte

| Decisione | Quando |
|---|---|
| RAW di prova: **deciso** — campioni scaricati online a licenza libera (raw.pixls.us), scelti per varietà: Bayer e X-Trans, 24 e ~48 MP, più marche (ARW, NEF, CR3, RAF, DNG). Il download va confermato file per file | elenco nel piano |
| Nome del prodotto: **deciso**, Unveil. Conflitti noti nel settore foto (unveil.camera, unveil.photo, Unveil – Personal Privacy); rischio accettato. Ricerca marchi rimandata alla pubblicazione | — |
