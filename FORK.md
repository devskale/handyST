# FORK.md — Wie dieser Fork gepflegt wird

Dieses Repo ist ein **long-lived fork von [cjpais/Handy](https://github.com/cjpais/Handy)**
(Standalone-Repo, kein GitHub-API-Fork). Branch `sttts` trägt die Fork-Entwicklung,
Upstream-Änderungen werden regelmäßig hineingemergt. Diese Datei ist der Fahrplan dafür —
sie ist **fork-eigen** (existiert nicht upstream) und kann daher nie einen Merge-Konflikt produzieren.

## Remotes (Einrichtung)

```bash
git remote add upstream https://github.com/cjpais/Handy.git   # read-only, https genügt
git fetch upstream
```

`origin` = `git@github.com:devskale/handyST.git`. Merges laufen NUR auf dem Mac-Clone;
der DGX-Clone pullt nur.

## Goldene Regeln

1. **Merge, nie rebase.** `sttts` ist gepusht und wird auf mehreren Maschinen geclont —
   History-Rewrite bricht jeden Clone. `git merge upstream/main`, fertig.
2. **Klein und regelmäßig mergen.** Fork-Divergenz verrottet exponentiell — 74 Commits
   Rückstand sind schon Arbeit, 200 sind eine Neuentwicklung. Lieber alle paar Wochen.
3. **Nur Merge-Commits, kein Squash** zwischen `sttts` und `upstream/main` —
   Squash zerstört die Konflikt-Auflösungs-Historie, auf die `rerere` aufbaut.
4. **Nie blind committen:** Merge mit `--no-commit` starten, Hotspots (unten) prüfen,
   Gates laufen lassen, erst dann committen.

## Merge-Workflow

```bash
git checkout sttts
git fetch upstream
git merge --no-commit upstream/main
#   → Konflikte lösen, dabei Hotspot-Checkliste unten durchgehen
bun run check:translations        # MUSS grün sein (siehe Vorfall unten)
bun run check:model-languages
bun run build                     # tsc && vite build
git commit                        # Merge-Message: Upstream-Range + Fork-Besonderheiten notieren
git push origin sttts
```

`rerere` ist aktiviert (`git config rerere.enabled true`, repo-lokal): wiederkehrende
Konflikte werden nach dem ersten korrekten Lösen aufgezeichnet und beim nächsten Merge
automatisch wiederverwendet. Gerade bei den i18n-Files zahlt das direkt ein.

## Konflikt-Hotspots — was jeden Merge ÜBERLEBEN muss

Diese Dinge sind in echten Merges schon verloren gegangen (Commits als Beleg):

| Was | Wo | Beleg-Vorfall |
|---|---|---|
| **Fork-i18n-Keys** | `src/i18n/locales/*/translation.json` | `509c8861` — der Upstream-Merge verlor die en-Keys → raw keys in der Settings-UI. Keys: `shortcutBehavior`, `idlePill`, `dictationShortcut`, `favoriteLanguages`, `sidebar.remoteTranscription`, `overlay.dictate`, `overlay.dragTooltip` |
| **`silero-vad-v6.2-F32.gguf`** | `src-tauri/resources/models/` | Merge 2026-11-18 (v0.10.1) — bis dahin verteidigte der Fork das alte `silero_vad_v4.onnx` (vad-rs); seit dem Entscheid unten läuft die VAD über transcribe-cpp und lädt das **gguf**. Ohne es schlägt die VAD-Init fehl |
| **Quit-Verhalten** | Main-Window-Close-Logik | `d6d3616d` — Upstream versteckt zu Tray; wir beenden die App (Pill muss mit weg) |
| **Pill-/Overlay-Verhalten** | Overlay-Anker, Transcript-Retention, Layout | `96159813`, `9a08d45b` — Upstream-Refactors der Overlay-Komponenten regelmäßig gegenprüfen |

Faustregel nach jedem Merge: `bun run check:translations` grün **und**
`ls src-tauri/resources/models/silero-vad-v6.2-F32.gguf` existiert **und**
`cargo test` (in `src-tauri/`) grün **und** App einmal starten
und Diktat + Pill + Fenster-close testen. `bun run check:model-languages` ist
**obsolet** (der Fork hat keine lokale Modell-Katalog-Infrastruktur mehr).

## Architektur-Entscheidungen (Upstream-Baum ≠ Fork-Baum)

Diese Punkte sind bewusste Divergenz — bei jedem Merge wieder in Richtung Fork auflösen,
nicht „reparieren“:

1. **Remote-only-Transkription.** Der Fork hat die komplette lokale Engine gelöscht
   (`engine_supervisor/`, `managers/model*`, `commands/models|transcription`, Model-Selector-UI,
   Modell-Katalog). `managers/transcription.rs` bleibt die Fork-Version (Gateway-Streaming via
   `remote_stream`/`remote_transcribe`); Upstream-Refactors daran (Transcription-Trait,
   Chunking-Jobs, `create_detector`-Nutzung) werden verworfen.
2. **VAD über transcribe-cpp/gguf (Entscheid 2026-11-18, Merge auf v0.10.1).** Bis dahin
   nutzte der Fork `vad-rs` + onnx nur für die VAD — ort komplett im Baum nur dafür. Jetzt
   folgt die VAD Upstream (`silero.rs` = upstream, transcribe.cpp-Backend) und ort ist raus.
   Die Fork-VAD-Fixes leben im backend-unabhängigen Smoothing-Layer (`smoothed.rs`: hangover
   pre-roll `ee89c9f0`, `flush_tail`), den Upstream nie anfasst — sie überleben jeden
   Backend-Wechsel. `Cargo.toml`: transcribe-cpp-Tabellen **ohne** vulkan/dynamic-backends
   (VAD ist CPU-only; spart Vulkan-SDK im Build), macOS-arm64 behält metal. Upstreams
   `handy-recorder`-Capture-Stack (CaptureSink/OpenMicrophone) wird NICHT übernommen —
   der Fork-Recorder (rtrb, Mic-Fallback, flush_tail-Aufruf) bleibt.
3. **Notices ohne Overlay.** Das Notices-Framework (upstream) ist übernommen — UI im
   Hauptfenster (`NoticePanel`, Footer-Chip) und Jobs-Events für History-Retry/File-Transkription
   (adaptiert: `transcription_jobs.rs` ruft `tm.transcribe()` statt dem Chunking-Trait, kein
   Model-Unload). Aber: das Fork-Overlay kennt keine Notice-States; `notices.rs::raise` zeigt
   Overlay-Notices bewusst nicht an.
4. **`language_detection` ist ein Typen-Stub.** Nur `LanguageDecision`/`LanguageSource`
   (Speicherformat der History-Einträge) existieren; Detektor/Katalog-Download bleibt
   upstream-only. `system_languages.rs` filtert nicht nach Modell-Katalog (remote: alles
   angeboten), Bokmål kollabiert auf `no`.
5. **Kein Onboarding-Flow, nur Accessibility-Schritt.** Upstreams neues Onboarding
   (#2303, Engine-Vorschau) braucht lokale Modelle — weg. Dafür nutzt
   `AccessibilityOnboarding` upstreams `reset_macos_permission` (#1618-Fix).
6. **Bindings.ts wird von Hand synchron gehalten** bis zum nächsten `tauri dev`-Lauf
   (Export passiert nur bei Debug-Builds): Notice-/Job-Typen, `spoken_languages`,
   `airpods_mode`, `HistoryEntry.source_name/language*` sind ergänzt. Nach dem ersten
   Dev-Lauf prüfen, ob die Regeneration sauber ist.

## Fork-Inventar (lebendig halten)

Was der Fork gegenüber Upstream trägt — jederzeit regenerieren:

```bash
git log upstream/main..sttts --no-merges --oneline
```

Momentan ~101 eigenständige Commits: Pill-UX, Remote-Transcription (Gateway-Streaming),
VAD-Fixes (hangover pre-roll, tail-flush), i18n-Ausbau, Overlay-Layout.

Klassiker, um veraltete Fork-Änderungen zu entschlacken (upstream hat sie übernommen oder
der Fork hat sie doppelt): `git log upstream/main..sttts --no-merges` durchgehen und prüfen,
ob Commits inzwischen obsolet sind. Der Heavy-Duty-Reset (upstream-Baum + Re-Cherry-Pick,
siehe „Git Tricks for Maintaining a Long-Lived Fork") ist optional und nur bei großer Rot ansetzen.

## Warum kein GitHub-API-Fork?

Ein bestehendes Repo lässt sich nicht in einen Fork umwandeln (nur detach, kein attach).
Der Nutzen (Compare-UI, Sync-Button, PR-Möglichkeit) wiegt den Repo-Wechsel-Churn nicht auf —
es geht kein PR nach upstream. Stand 2026-11 bewusst so entschieden.
