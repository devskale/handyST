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
| **`silero_vad_v4.onnx`** | `src-tauri/resources/models/` | `04258f76` — Upstream ersetzt es gern durch die v6.2-gguf (transcribe.cpp-VAD); unser Recorder (`src-tauri/src/audio_toolkit/vad/silero.rs`) lädt das **onnx**. Ohne das onnx schlägt die VAD-Init fehl |
| **Quit-Verhalten** | Main-Window-Close-Logik | `d6d3616d` — Upstream versteckt zu Tray; wir beenden die App (Pill muss mit weg) |
| **Pill-/Overlay-Verhalten** | Overlay-Anker, Transcript-Retention, Layout | `96159813`, `9a08d45b` — Upstream-Refactors der Overlay-Komponenten regelmäßig gegenprüfen |

Faustregel nach jedem Merge: `bun run check:translations` grün **und**
`ls src-tauri/resources/models/silero_vad_v4.onnx` existiert **und** App einmal starten
und Diktat + Pill + Fenster-close testen.

## Fork-Inventar (lebendig halten)

Was der Fork gegenüber Upstream trägt — jederzeit regenerieren:

```bash
git log upstream/main..sttts --no-merges --oneline
```

Momentan ~97 eigenständige Commits: Pill-UX, Remote-Transcription (Gateway-Streaming),
VAD-Fixes (hangover pre-roll, tail-flush), i18n-Ausbau, Overlay-Layout.

Klassiker, um veraltete Fork-Änderungen zu entschlacken (upstream hat sie übernommen oder
der Fork hat sie doppelt): `git log upstream/main..sttts --no-merges` durchgehen und prüfen,
ob Commits inzwischen obsolet sind. Der Heavy-Duty-Reset (upstream-Baum + Re-Cherry-Pick,
siehe „Git Tricks for Maintaining a Long-Lived Fork") ist optional und nur bei großer Rot ansetzen.

## Warum kein GitHub-API-Fork?

Ein bestehendes Repo lässt sich nicht in einen Fork umwandeln (nur detach, kein attach).
Der Nutzen (Compare-UI, Sync-Button, PR-Möglichkeit) wiegt den Repo-Wechsel-Churn nicht auf —
es geht kein PR nach upstream. Stand 2026-11 bewusst so entschieden.
