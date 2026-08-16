//! macOS-dictation-style triggers: start/stop dictation without a chord.
//!
//! Supported triggers (Settings → General → Dictation shortcut):
//!   mic_key      — the dedicated mic/dictation key (F5 position, keycode 0xB0)
//!   ctrl_double  — tap Control twice
//!   cmd_left_double / cmd_right_double — tap one Command side twice
//!   globe_double — tap the Globe/Fn key twice
//!
//! Double-tap detection watches raw `changed_modifier` events: a tap only
//! counts while no other key or modifier goes down in between, so real
//! shortcuts (Ctrl+C, Cmd+Tab, …) never fire a trigger. Triggers always use
//! toggle semantics (tap = start, tap = stop) independent of the
//! hold-vs-click recording mode.

use handy_keys::{Key, KeyboardListener, Modifiers};
use log::{debug, info, warn};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

use crate::transcription_coordinator::TranscriptionCoordinator;
use tauri::Manager;

/// Max time between the two taps of a double-tap.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(500);
/// Poll cadence of the listener thread.
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// The persisted trigger kinds (serde-friendly strings, see settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    MicKey,
    CtrlDouble,
    CmdLeftDouble,
    CmdRightDouble,
    OptLeftDouble,
    OptRightDouble,
    GlobeDouble,
}

impl Trigger {
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "mic_key" => Some(Self::MicKey),
            "ctrl_double" => Some(Self::CtrlDouble),
            "cmd_left_double" => Some(Self::CmdLeftDouble),
            "cmd_right_double" => Some(Self::CmdRightDouble),
            "opt_left_double" => Some(Self::OptLeftDouble),
            "opt_right_double" => Some(Self::OptRightDouble),
            "globe_double" => Some(Self::GlobeDouble),
            _ => None,
        }
    }

    fn watched_modifiers(&self) -> Modifiers {
        match self {
            Self::CtrlDouble => Modifiers::CTRL,
            Self::CmdLeftDouble => Modifiers::CMD_LEFT,
            Self::CmdRightDouble => Modifiers::CMD_RIGHT,
            Self::OptLeftDouble => Modifiers::OPT_LEFT,
            Self::OptRightDouble => Modifiers::OPT_RIGHT,
            Self::GlobeDouble => Modifiers::FN,
            Self::MicKey => Modifiers::empty(),
        }
    }

    fn matches_mic_key(&self, key: Option<Key>) -> bool {
        matches!(self, Self::MicKey) && matches!(key, Some(Key::Dictation))
    }
}

/// Pure double-tap state machine over the watched modifier. Testable without
/// a real keyboard: feed events, `true` means "trigger fired".
struct DoubleTapDetector {
    last_tap: Option<Instant>,
    /// Set when any other key/modifier event happened since the pending tap.
    dirty: bool,
}

impl DoubleTapDetector {
    fn new() -> Self {
        Self {
            last_tap: None,
            dirty: false,
        }
    }

    fn reset(&mut self) {
        self.last_tap = None;
        self.dirty = false;
    }

    /// A key-down of the watched modifier.
    fn on_watched_tap(&mut self, now: Instant) -> bool {
        if self.dirty {
            self.reset();
            self.last_tap = Some(now);
            return false;
        }
        match self.last_tap {
            Some(t) if now.duration_since(t) <= DOUBLE_TAP_WINDOW => {
                self.reset();
                true
            }
            _ => {
                self.last_tap = Some(now);
                false
            }
        }
    }

    /// Any unrelated key or modifier activity invalidates a pending tap.
    fn on_other_activity(&mut self) {
        if self.last_tap.is_some() {
            self.dirty = true;
        }
    }

    /// Expire stale taps so an hour-old single press can't pair with a new one.
    fn tick(&mut self, now: Instant) {
        if let Some(t) = self.last_tap {
            if now.duration_since(t) > DOUBLE_TAP_WINDOW {
                self.reset();
            }
        }
    }
}

/// Commands for the listener thread.
enum Command {
    SetTrigger(Option<Trigger>),
    Shutdown,
}

static COMMAND_TX: OnceLock<Sender<Command>> = OnceLock::new();
/// Current trigger as a small discriminant the listener can hot-swap.
static ACTIVE_TRIGGER: AtomicU64 = AtomicU64::new(0);

fn trigger_code(t: Option<Trigger>) -> u64 {
    match t {
        None => 0,
        Some(Trigger::MicKey) => 1,
        Some(Trigger::CtrlDouble) => 2,
        Some(Trigger::CmdLeftDouble) => 3,
        Some(Trigger::CmdRightDouble) => 4,
        Some(Trigger::OptLeftDouble) => 6,
        Some(Trigger::OptRightDouble) => 7,
        Some(Trigger::GlobeDouble) => 5,
    }
}

fn current_trigger() -> Option<Trigger> {
    match ACTIVE_TRIGGER.load(Ordering::Relaxed) {
        1 => Some(Trigger::MicKey),
        2 => Some(Trigger::CtrlDouble),
        3 => Some(Trigger::CmdLeftDouble),
        4 => Some(Trigger::CmdRightDouble),
        5 => Some(Trigger::GlobeDouble),
        6 => Some(Trigger::OptLeftDouble),
        7 => Some(Trigger::OptRightDouble),
        _ => None,
    }
}

/// Start the listener thread (once). Called from app setup.
pub fn init(app: &AppHandle) {
    let (tx, rx) = mpsc::channel::<Command>();
    if COMMAND_TX.set(tx).is_err() {
        return; // already initialized
    }

    let app = app.clone();
    thread::Builder::new()
        .name("dictation-shortcut".into())
        .spawn(move || {
            let listener = match KeyboardListener::new() {
                Ok(l) => l,
                Err(e) => {
                    warn!("dictation shortcut: keyboard listener unavailable: {e}");
                    return;
                }
            };
            info!("dictation shortcut listener started");

            let mut detector = DoubleTapDetector::new();
            loop {
                // Drain commands without blocking the event stream for long.
                if let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        Command::SetTrigger(t) => {
                            ACTIVE_TRIGGER.store(trigger_code(t), Ordering::Relaxed);
                            detector.reset();
                            info!("dictation shortcut set to {t:?}");
                        }
                        Command::Shutdown => break,
                    }
                }

                let event = listener.try_recv();
                if let Some(event) = event {
                    let Some(trigger) = current_trigger() else {
                        continue;
                    };
                    let watched = trigger.watched_modifiers();

                    if event.is_key_down && trigger.matches_mic_key(event.key) {
                        fire(&app);
                        continue;
                    }

                    if let Some(changed) = event.changed_modifier {
                        let watched_hit = !watched.is_empty() && changed.intersects(watched);
                        if event.is_key_down && watched_hit {
                            if detector.on_watched_tap(Instant::now()) {
                                fire(&app);
                            }
                        } else if event.is_key_down {
                            // A different modifier went down mid-tap.
                            detector.on_other_activity();
                        }
                        // Releases of the watched modifier are expected
                        // between taps and keep the tap pending.
                        continue;
                    }

                    if event.is_key_down {
                        // Any regular key press invalidates a pending tap
                        // (it is probably part of a real shortcut).
                        detector.on_other_activity();
                    }
                } else {
                    thread::sleep(POLL_TIMEOUT);
                    detector.tick(Instant::now());
                }
            }
            info!("dictation shortcut listener stopped");
        })
        .ok();
}

/// Apply the persisted setting (startup + on change).
pub fn apply_setting(value: &str) {
    if let Some(tx) = COMMAND_TX.get() {
        let _ = tx.send(Command::SetTrigger(Trigger::from_setting(value)));
    }
}

pub fn shutdown() {
    if let Some(tx) = COMMAND_TX.get() {
        let _ = tx.send(Command::Shutdown);
    }
}

/// Toggle transcription with toggle semantics regardless of the recording
/// mode — a tap trigger has no meaningful "release".
fn fire(app: &AppHandle) {
    if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
        debug!("dictation shortcut fired — toggling transcription");
        coordinator.send_input("transcribe", "dictation-trigger", true, false);
    } else {
        warn!("dictation shortcut: TranscriptionCoordinator not initialized");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> DoubleTapDetector {
        DoubleTapDetector::new()
    }

    #[test]
    fn two_quick_taps_fire() {
        let mut d = detector();
        let t0 = Instant::now();
        assert!(!d.on_watched_tap(t0));
        assert!(d.on_watched_tap(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn slow_taps_do_not_fire() {
        let mut d = detector();
        let t0 = Instant::now();
        assert!(!d.on_watched_tap(t0));
        assert!(!d.on_watched_tap(t0 + Duration::from_millis(900)));
    }

    #[test]
    fn intervening_key_press_blocks_fire() {
        let mut d = detector();
        let t0 = Instant::now();
        assert!(!d.on_watched_tap(t0));
        d.on_other_activity(); // e.g. the "C" in Ctrl+C
        // The shortcut-paired press must NOT complete the double-tap…
        assert!(!d.on_watched_tap(t0 + Duration::from_millis(100)));
        // …but it becomes a fresh first tap, so a following clean tap fires —
        // two real taps after the shortcut are a genuine double-tap (macOS
        // behaves the same way).
        assert!(d.on_watched_tap(t0 + Duration::from_millis(300)));
    }

    #[test]
    fn firing_resets_state() {
        let mut d = detector();
        let t0 = Instant::now();
        assert!(!d.on_watched_tap(t0));
        assert!(d.on_watched_tap(t0 + Duration::from_millis(100)));
        // A single later tap must not fire immediately again.
        assert!(!d.on_watched_tap(t0 + Duration::from_millis(300)));
    }

    #[test]
    fn trigger_parsing_roundtrip() {
        assert_eq!(Trigger::from_setting("mic_key"), Some(Trigger::MicKey));
        assert_eq!(Trigger::from_setting("globe_double"), Some(Trigger::GlobeDouble));
        assert_eq!(Trigger::from_setting("opt_right_double"), Some(Trigger::OptRightDouble));
        assert_eq!(Trigger::from_setting("none"), None);
        assert_eq!(Trigger::from_setting(""), None);
    }
}
