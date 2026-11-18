#!/usr/bin/env bash
# loop-test.sh — automatisierter Smoke-Loop für den handyST-Fork
#
# Automatisiert die FORK.md-Checkliste („App einmal starten und Diktat + Pill +
# Fenster-close testen") als N-fachen Loop:
#
#   1. App (Debug-Bundle, isolierter Portable-Datenordner) mit --pill-test
#      starten → Verdict-JSON abwarten (Pill: show → move → persist → restore)
#   2. Diktat per CLI-Remote-Flag an die laufende Instanz togglen (an/aus) —
#      die App muss das ohne Crash überleben
#   3. Hauptfenster per AppleScript schließen → der Prozess MUSS sich beenden
#      (Fork-Verhalten: close quits, die Pill geht mit weg)
#
# Der Test läuft gegen einen isolierten Portable-Modus (Marker + Data/ neben
# dem Binary im Debug-Bundle) — er berührt weder die echten App-Daten noch
# die echten Shortcuts des Entwicklers.
#
# Usage:
#   scripts/loop-test.sh [N]            # N Iterationen (default 3)
#   scripts/loop-test.sh --build [N]    # Debug-Bundle vorher neu bauen
#
# Env:
#   LOOP_TEST_EXPECT_AUDIO=1  verlangt fehlerfreien Recording-Start im Log
#                             (Mikrofonrechte müssen granted sein)
#   LOOP_TEST_KEEP_APP=1      App nach dem Loop nicht schließen (Debugging)
#
# Gates in FORK.md-Sprache: exit 0 = alle Iterationen grün.

set -uo pipefail

APP_NAME="handyST"
ITERATIONS=3
BUILD=false
for arg in "$@"; do
  case "$arg" in
    --build) BUILD=true ;;
    [0-9]*) ITERATIONS="$arg" ;;
    *) echo "unbekanntes Argument: $arg (erwartet: [N] | --build)"; exit 2 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_BUNDLE="$REPO_ROOT/src-tauri/target/debug/bundle/macos/$APP_NAME.app"
BIN="$APP_BUNDLE/Contents/MacOS/$APP_NAME"
MACOS_DIR="$APP_BUNDLE/Contents/MacOS"
PORTABLE_MARKER="$MACOS_DIR/portable"
DATA_DIR="$MACOS_DIR/Data"
LOG_FILE="$DATA_DIR/logs/handy.log"
VERDICT="$DATA_DIR/pill_test_result.json"

fail()  { echo "  ✗ FAIL: $*" >&2; return 1; }
pass()  { echo "  ✓ $*"; }

# ── 0. Debug-Bundle ──────────────────────────────────────────────────────────
if $BUILD || [ ! -x "$BIN" ]; then
  echo "→ baue Debug-Bundle (einmalig, dauert)…"
  # Updater-Artifacts brauchen TAURI_SIGNING_PRIVATE_KEY — im Debug-Loop aus.
  (cd "$REPO_ROOT" && bun run tauri build --debug --bundles app \
      --config '{"bundle":{"createUpdaterArtifacts":false}}') > /tmp/loop-test-build.log 2>&1
  rc=$?
  if [ $rc -ne 0 ] || [ ! -x "$BIN" ]; then
    tail -20 /tmp/loop-test-build.log
    echo "✗ Build fehlgeschlagen (rc=$rc), Log: /tmp/loop-test-build.log"; exit 1
  fi
  pass "Debug-Bundle gebaut"
fi

# ── 1. Isolierter Portable-Modus ─────────────────────────────────────────────
# Marker + Data/ neben dem Binary lenken app_data_dir + logs in den Bundle-
# Ordner (portable::init erkennt den Marker). Bestehende Testdaten bleiben
# zwischen Iterationen erhalten (Settings/Shortcut-Grants), frische Verdicts
# werden pro Iteration gelöscht.
printf 'Handy Portable Mode' > "$PORTABLE_MARKER"
mkdir -p "$DATA_DIR/logs"
pass "Portable-Isolation aktiv: $DATA_DIR"

# ── Helfer ───────────────────────────────────────────────────────────────────
app_pid() {
  pgrep -f "$BIN" | head -1
}

wait_for_verdict() {
  local deadline=$((SECONDS + 25))
  while [ $SECONDS -lt $deadline ]; do
    if [ -s "$VERDICT" ]; then
      # JSON-Plausibilität (ok-Feld) — python ist auf macOS immer da
      if python3 -c "
import json,sys
d=json.load(open('$VERDICT'))
sys.exit(0 if d.get('ok') is True else 1)
" 2>/dev/null; then return 0; else return 1; fi
    fi
    sleep 0.5
  done
  return 1
}

wait_pid_gone() {
  local pid="$1" deadline=$((SECONDS + 8))
  while [ $SECONDS -lt $deadline ]; do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.3
  done
  return 1
}

close_main_window() {
  local pid="$1"
  # Hauptfenster anhand des Namens schließen — window 1 wäre ggf. die Pill
  # („Recording“), deren Close nicht die App beendet. Fällt auf Cmd+W zurück,
  # nachdem das Hauptfenster fokussiert wurde.
  osascript >/dev/null 2>&1 <<OSA && return 0
tell application "System Events"
  tell (first process whose unix id is $pid)
    click button 1 of window "handyST"
  end tell
end tell
OSA
  osascript >/dev/null 2>&1 <<OSA
tell application "System Events"
  tell (first process whose unix id is $pid)
    perform action "AXRaise" of window "handyST"
  end tell
end tell
OSA
  osascript -e 'tell application "System Events" to keystroke "w" using command down' >/dev/null 2>&1
}

log_tail() { tail -n "${1:-40}" "$LOG_FILE" 2>/dev/null || true; }

# ── 2. Der Loop ──────────────────────────────────────────────────────────────
echo "→ $ITERATIONS Iterationen gegen $BIN"
FAILED=0

for i in $(seq 1 "$ITERATIONS"); do
  echo "── Iteration $i/$ITERATIONS ──"
  ITER_FAIL=0

  rm -f "$VERDICT"
  : > /tmp/loop-test-it-$i.log

  # (a) Start mit Pill-Selbsttest — das Verdict ist zugleich der Ready-Marker
  "$BIN" --pill-test >> "/tmp/loop-test-it-$i.log" 2>&1 &
  sleep 1
  PID="$(app_pid)"
  if [ -z "$PID" ]; then
    fail "App-Start: Prozess nicht am Leben" && ITER_FAIL=1
  else
    pass "App gestartet (pid $PID)"
    if wait_for_verdict; then
      pass "Pill-Selbsttest ok (show → move → persist → restore)"
    else
      fail "Pill-Verdict nicht ok / nicht erschienen: $VERDICT" && ITER_FAIL=1
      log_tail 30 | sed 's/^/      log: /'
    fi
  fi

  # (b) Diktat an/aus per Remote-Flag; die Instanz muss beide Toggles überleben
  if [ "$ITER_FAIL" -eq 0 ] && [ -n "$PID" ]; then
    "$BIN" --toggle-transcription >/dev/null 2>&1
    sleep 2
    if kill -0 "$PID" 2>/dev/null; then
      pass "Diktat-Toggle an: App überlebt"
    else
      fail "Diktat-Toggle an: App ist gestorben" && ITER_FAIL=1
    fi

    if [ "${LOOP_TEST_EXPECT_AUDIO:-0}" = "1" ]; then
      if log_tail 80 | grep -qE "recording-error|Failed to start recording|microphone_permission_denied"; then
        fail "Erwarteter Audio-Start, aber Recording-Fehler im Log" && ITER_FAIL=1
      else
        pass "Kein Recording-Fehler im Log (LOOP_TEST_EXPECT_AUDIO)"
      fi
    fi

    "$BIN" --toggle-transcription >/dev/null 2>&1
    sleep 1
    if kill -0 "$PID" 2>/dev/null; then
      pass "Diktat-Toggle aus: App überlebt"
    else
      fail "Diktat-Toggle aus: App ist gestorben" && ITER_FAIL=1
    fi
  fi

  # (c) Fenster schließen → Prozess MUSS enden (Quit-Verhalten; die Pill ist
  #     derselbe Prozess — Prozess-Ende deckt „Pill geht mit weg" ab)
  if [ "$ITER_FAIL" -eq 0 ] && [ -n "$PID" ]; then
    close_main_window "$PID"
    if wait_pid_gone "$PID"; then
      pass "Fenster-Close beendet die App (Quit-Verhalten)"
    else
      fail "App lebt noch nach Fenster-Close (Upstream-Verhalten eingeschlichen?)"
      ITER_FAIL=1
    fi
  fi

  # Aufräumen, falls etwas hängen blieb
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    if [ "${LOOP_TEST_KEEP_APP:-0}" != "1" ]; then
      kill -9 "$PID" 2>/dev/null; sleep 1
    fi
  fi

  if [ "$ITER_FAIL" -eq 0 ]; then
    echo "  → Iteration $i grün"
  else
    echo "  → Iteration $i ROT (Log: /tmp/loop-test-it-$i.log, App-Log: $LOG_FILE)"
    FAILED=$((FAILED + 1))
  fi
done

# ── 3. Summary ───────────────────────────────────────────────────────────────
echo "────────────────────────────────────"
if [ "$FAILED" -eq 0 ]; then
  echo "✓ Looptest grün: $ITERATIONS/$ITERATIONS Iterationen"
  exit 0
else
  echo "✗ Looptest rot: $FAILED/$ITERATIONS Iterationen fehlgeschlagen"
  exit 1
fi
