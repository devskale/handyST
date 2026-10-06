import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { availableMonitors } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";
import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import "./RecordingOverlay.css";
import { commands, events } from "@/bindings";
import type {
  StreamPhase,
  StreamPhaseEvent,
  StreamTextEvent,
  StreamWorkKind,
} from "@/bindings";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import { DEFAULT_FAVORITE_LANGUAGES } from "@/lib/constants/languages";
import { getLanguageDirection } from "@/lib/utils/rtl";

type OverlayState =
  | "recording"
  | "streaming"
  | "transcribing"
  | "processing"
  | "complete"
  | "idle";

// Number of reactive bars in the waveform (the simple, smoothed style shared by
// every overlay form). Mic levels arrive as 16 FFT buckets; we take the first N.
const WAVE_BARS = 9;

const RecordingOverlay: React.FC = () => {
  const pillDrag = usePillDrag();
  const { t } = useTranslation();
  const [isVisible, setIsVisible] = useState(false);
  const [state, setState] = useState<OverlayState>("recording");
  // `Stream::play()` returning does not mean hardware callbacks are flowing.
  // Stay visually in an arming state until the backend processes the first
  // actual microphone sample chunk.
  const [captureReady, setCaptureReady] = useState(false);
  const [levels, setLevels] = useState<number[]>(Array(WAVE_BARS).fill(0));
  const [streamText, setStreamText] = useState<StreamTextEvent>({
    committed: "",
    tentative: "",
  });
  const [phase, setPhase] = useState<StreamPhase>("listening");
  const [workKind, setWorkKind] = useState<StreamWorkKind>("transcribing");
  const [elapsed, setElapsed] = useState(0);
  // Bumped on each new streaming session so the Live card remounts fresh (replays
  // the pop-in, and never animates in from the previous panel's open size).
  const [session, setSession] = useState(0);
  // Overlay placement (top vs bottom of the screen). The Live panel grows downward
  // from a top overlay (oldest line under the pill) and upward from a bottom one.
  const [position, setPosition] = useState<"top" | "bottom">("bottom");
  // True once live text overflows the cap. A top overlay fades its top edge only
  // while overflowing, so the resting first line stays crisp flush under the pill.
  const [overflowing, setOverflowing] = useState(false);
  // Language quick-switcher: favorites + the current selection. Applies to
  // the next utterance (the stream is opened with it as the language hint).
  const [favLanguages, setFavLanguages] =
    useState<string[]>(DEFAULT_FAVORITE_LANGUAGES);
  const [curLanguage, setCurLanguage] = useState("auto");

  const switchLanguage = (lang: string) => {
    if (lang === curLanguage) return;
    setCurLanguage(lang);
    commands.changeSelectedLanguageSetting(lang).catch((e) => {
      console.warn("Failed to set language:", e);
    });
  };

  const smoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  // Live-text scroll-back: the text region "sticks" to the newest line while the
  // user is at the bottom; if they scroll up to read history, auto-follow pauses
  // until they scroll back down.
  const capRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const direction = getLanguageDirection(i18n.language);

  useEffect(() => {
    const setupEventListeners = async () => {
      const unlistenShow = await listen("show-overlay", async (event) => {
        const overlayState = event.payload as OverlayState;
        // Reset synchronously before settings I/O. A fast microphone can emit
        // recording-ready while the awaits below are in flight; resetting after
        // them would overwrite that event and leave the overlay stuck arming.
        if (overlayState === "recording" || overlayState === "streaming") {
          setCaptureReady(false);
          smoothedLevelsRef.current = Array(16).fill(0);
          setLevels(Array(WAVE_BARS).fill(0));
          setStreamText({ committed: "", tentative: "" });
        }

        await syncLanguageFromSettings();
        // The Live panel flows downward from a top overlay and upward from a
        // bottom one; read the placement so the layout can flip to match.
        try {
          const settings = await commands.getAppSettings();
          if (settings.status === "ok") {
            setPosition(
              settings.data.overlay_position === "top" ? "top" : "bottom",
            );
            if (settings.data.favorite_languages?.length) {
              setFavLanguages(settings.data.favorite_languages);
            }
            setCurLanguage(settings.data.selected_language || "auto");
          }
        } catch {
          // Keep the previous/default placement if settings can't be read.
        }
        setState(overlayState);
        if (overlayState === "streaming") {
          setPhase("listening");
          setWorkKind("transcribing");
          setElapsed(0);
          setSession((s) => s + 1); // remount the card fresh for this session
        }
        setIsVisible(true);
      });

      const unlistenHide = await listen("hide-overlay", () => {
        setIsVisible(false);
        setCaptureReady(false);
      });

      const unlistenReady = await listen("recording-ready", () => {
        setElapsed(0);
        setCaptureReady(true);
      });

      const unlistenLevel = await listen<number[]>("mic-level", (event) => {
        const newLevels = event.payload as number[];
        // Exponential smoothing across the 16 buckets, then take the first N
        // bars for the shared waveform.
        const smoothed = smoothedLevelsRef.current.map((prev, i) => {
          const target = newLevels[i] || 0;
          return prev * 0.7 + target * 0.3;
        });
        smoothedLevelsRef.current = smoothed;
        setLevels(smoothed.slice(0, WAVE_BARS));
      });

      const unlistenStream = await events.streamTextEvent.listen((event) => {
        setStreamText(event.payload);
      });

      // sttts: the webview can finish loading AFTER the backend emitted the
      // startup show-overlay ("idle") event — the window then sits invisible
      // and silently swallows clicks. Ask for the current state on mount.
      try {
        await commands.showIdlePillIfEnabled();
      } catch (e) {
        console.warn("sttts: overlay state re-request failed:", e);
      }

      const unlistenPhase = await events.streamPhaseEvent.listen((event) => {
        const payload: StreamPhaseEvent = event.payload;
        setPhase(payload.phase);
        if (payload.kind) setWorkKind(payload.kind);
      });

      return () => {
        unlistenShow();
        unlistenHide();
        unlistenReady();
        unlistenLevel();
        unlistenStream();
        unlistenPhase();
      };
    };

    setupEventListeners();
  }, []);

  // Elapsed capture timer starts only once microphone samples are flowing.
  useEffect(() => {
    if (state !== "streaming" || !isVisible || !captureReady) return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [state, isVisible, captureReady]);

  // Stick to the bottom as text streams in — but only while pinned, so a user who
  // has scrolled up to read history isn't yanked back down by the next chunk.
  useLayoutEffect(() => {
    const el = capRef.current;
    if (!el) return;
    // Fade the top edge only once text actually overflows the cap.
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    if (pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [streamText]);

  // Each fresh streaming session starts pinned to the bottom, fade cleared.
  useEffect(() => {
    pinnedRef.current = true;
    setOverflowing(false);
  }, [session]);

  // The complete pill lingers a few seconds with a "Dictate again" button,
  // then fades on its own — and hands over to the persistent idle pill.
  useEffect(() => {
    if (state !== "complete" || !isVisible) return;
    const id = setTimeout(() => {
      commands.showIdlePillIfEnabled();
      setIsVisible(false);
    }, 5000);
    return () => clearTimeout(id);
  }, [state, isVisible, session]);

  if (!isVisible) return null;

  // Re-pin when the user is within ~a line of the bottom; unpin otherwise.
  const handleStreamScroll = () => {
    const el = capRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 16;
  };

  const fmtTime = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  // ---- Shared building blocks (one visual language for every overlay form) ----
  const waveform = (
    <div className={`swave ${captureReady ? "ready" : "arming"}`}>
      {levels.map((v, i) => (
        <i
          key={i}
          style={{
            height: `${Math.max(3, Math.min(18, 3 + Math.pow(v, 0.7) * 15))}px`,
          }}
        />
      ))}
    </div>
  );

  const cancelBtn = (
    <button
      className="sx"
      aria-label="cancel"
      onClick={() => commands.cancelOperation()}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M4 4 L12 12 M12 4 L4 12"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
        />
      </svg>
    </button>
  );

  // sttts: the drag handle — identical in every overlay state, so the pill is
  // movable while idle, recording, streaming and finishing alike.
  const pillGrip = (
    <div
      className="pill-grip"
      title="Ziehen zum Verschieben"
      onPointerDown={pillDrag.onPointerDown}
    >
      <span />
      <span />
      <span />
    </div>
  );

  // dot (left) | waveform (center) | timer + cancel (right) — same structure for
  // pill & panel, so the Live morph is a pure width change.
  const listeningRow = (showTimer: boolean, showCancel: boolean) => (
    <div className="sbase">
      {pillGrip}
      <div className="sbase-l">
        <span className={`sdot ${captureReady ? "ready" : "arming"}`} />
      </div>
      {waveform}
      <div className="sbase-r">
        {showTimer && <span className="stimer">{fmtTime(elapsed)}</span>}
        {showCancel && cancelBtn}
      </div>
    </div>
  );

  // spinner (left) | label (center) | cancel (right) — same 3-zone grid as the
  // listening row, so the label is centered.
  const workingRow = (label: string, showCancel: boolean) => (
    <div className="sbase">
      {pillGrip}
      <div className="sbase-l">
        <span className="sspinner" />
      </div>
      <span className="swork-label">{label}</span>
      <div className="sbase-r">{showCancel && cancelBtn}</div>
    </div>
  );

  // ---- Live overlay: a pill that sculpts open into a panel ----
  if (state === "streaming") {
    const hasText =
      streamText.committed.length > 0 || streamText.tentative.length > 0;
    const working = phase === "working";
    // Keep the panel open whenever there's text — even while finalizing — so the
    // transcript stays put under a working spinner instead of collapsing and
    // squishing the text mid-stream. Only fall back to the small working pill
    // when there was no text to preserve.
    const open = hasText;
    const collapsed = working && !hasText;

    return (
      <div dir={direction} className={`ov-stage ${position}`}>
        <div
          key={session}
          className={`scard ${open ? "open" : ""} ${collapsed ? "working" : ""} ${
            isVisible ? "" : "leaving"
          }`}
        >
          <div className="stext">
            <div className="stext-clip">
              <div
                className={`stext-cap ${overflowing ? "overflowing" : ""}`}
                ref={capRef}
                onScroll={handleStreamScroll}
              >
                <p>
                  <span className="committed">
                    {streamText.committed ? streamText.committed + " " : ""}
                  </span>
                  <span className="tentative">{streamText.tentative}</span>
                  {/* Drop the blinking caret once finalizing — it's no longer
                      capturing, and a static spinner conveys the work. */}
                  {!working && <span className="scaret" />}
                </p>
              </div>
            </div>
          </div>
          <div className="slang-row" dir="ltr">
            {favLanguages.map((lang) => (
              <button
                key={lang}
                className={`slang-chip ${lang === curLanguage ? "active" : ""}`}
                onClick={() => switchLanguage(lang)}
                title={
                  lang === "auto"
                    ? t("overlay.languageAutoTooltip")
                    : t("overlay.languageForceTooltip", {
                        lang: lang.toUpperCase(),
                      })
                }
              >
                {lang === "auto" ? "AUTO" : lang.toUpperCase()}
              </button>
            ))}
          </div>
          {working
            ? workingRow(
                workKind === "polishing"
                  ? t("overlay.processing")
                  : t("overlay.transcribing"),
                true,
              )
            : listeningRow(open, true)}
        </div>
      </div>
    );
  }

  // ---- Idle pill: persistent, click to dictate (sttts) ----
  if (state === "idle") {
    return (
      <div
        dir={direction}
        className={`ov-stage ${position} ov-fade ${isVisible ? "show" : ""}`}
      >
        <div className="scard compact">
          <div className="pill-row">
            {pillGrip}
            <button
              className="sbase again-btn"
            onClick={() => {
              setIsVisible(false);
              commands.toggleDictation();
            }}
          >
            <span className="sbase-l">
              <span className="sdot" />
            </span>
            {/* eslint-disable-next-line i18next/no-literal-string -- action verb */}
            <span className="swork-label">Dictate</span>
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ---- Complete pill: "Dictate again" after a finished dictation ----
  if (state === "complete") {
    return (
      <div
        dir={direction}
        className={`ov-stage ${position} ov-fade ${isVisible ? "show" : ""}`}
      >
        <div className="scard compact cworking">
          <div className="pill-row">
          {pillGrip}
          <button
            className="sbase again-btn"
            onClick={() => {
              setIsVisible(false);
              commands.toggleDictation();
            }}
          >
            <span className="sbase-l">
              <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
                <path
                  d="M8 2a6 6 0 1 1-5.66 4H4.2A4.6 4.6 0 1 0 8 3.4V6L4.5 3 8 0v2z"
                  fill="currentColor"
                />
              </svg>
            </span>
            {/* eslint-disable-next-line i18next/no-literal-string -- action verb */}
            <span className="swork-label">Dictate</span>
          </button>
          </div>
        </div>
      </div>
    );
  }

  // ---- Minimal overlay: exactly one row at a time — waveform (recording), or a
  // spinner + label (transcribing / processing). Never both. The pill animates its
  // width between them; the cancel button is in both rows so it stays put.
  const working = state === "transcribing" || state === "processing";
  const workLabel =
    state === "processing"
      ? t("overlay.processing")
      : t("overlay.transcribing");

  return (
    <div
      dir={direction}
      className={`ov-stage ${position} ov-fade ${isVisible ? "show" : ""}`}
    >
      <div
        className={`scard compact ${working && isVisible ? "cworking" : ""}`}
      >
        {working ? workingRow(workLabel, true) : listeningRow(false, true)}
      </div>
    </div>
  );
};


/**
 * sttts: drag handling for the pill/overlay window. pointerdown hands the
 * window to the OS-native drag (startDragging) — no manual position math,
 * which used to fight the backend's show-time repositioning and jump around.
 * A plain click without movement still reaches the buttons (distance-checked
 * in the click-capture phase). Once the window settles, the position is
 * persisted (logical coordinates) and wins over the centered default.
 */
const PILL_CLICK_SUPPRESS_PX = 6;

function usePillDrag() {
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    // Buttons (Dictate, language chips, cancel) must keep their normal click:
    // a native drag session swallows the mouse-up, so the click never fires.
    if ((e.target as HTMLElement).closest("button")) return;
    const startX = e.clientX;
    const startY = e.clientY;
    void commands.setOverlayPillDragging(true);
    void getCurrentWebviewWindow().startDragging();

    // A dragged release must not also fire the pill's click-to-dictate.
    const suppress = (click: MouseEvent) => {
      window.removeEventListener("click", suppress, true);
      if (
        Math.hypot(click.clientX - startX, click.clientY - startY) >
        PILL_CLICK_SUPPRESS_PX
      ) {
        click.stopPropagation();
        click.preventDefault();
      }
    };
    window.addEventListener("click", suppress, true);
    setTimeout(() => window.removeEventListener("click", suppress, true), 2000);

    // The OS owns the window during the drag; pointerup may not be delivered
    // to the webview. Poll until the position is stable, then persist once.
    let last: string | null = null;
    let stable = 0;
    let tries = 0;
    const iv = window.setInterval(async () => {
      tries++;
      try {
        const win = getCurrentWebviewWindow();
        let pos = await win.position();

        // sttts: soft walls — the OS drag happily moves the window past the
        // screen edge, where it gets clipped pixel by pixel ("sie
        // verschwindet langsam"). Clamp against the union of all displays so
        // the whole window stays in visible space, on every poll.
        const [scale, monitors] = await Promise.all([
          win.scaleFactor(),
          availableMonitors(),
        ]);
        if (monitors.length > 0) {
          const minX = Math.min(...monitors.map((m) => m.position.x));
          const minY = Math.min(...monitors.map((m) => m.position.y));
          const maxX = Math.max(
            ...monitors.map((m) => m.position.x + m.size.width),
          );
          const maxY = Math.max(
            ...monitors.map((m) => m.position.y + m.size.height),
          );
          // Card-aware margins: the visible card sits 42px inside the window,
          // so the window may overhang an edge by that much without the card
          // clipping.
          const inset = 42 * scale;
          const cx = Math.min(
            Math.max(pos.x, minX - inset),
            Math.max(minX - inset, maxX - (256 * scale - inset)),
          );
          const cy = Math.min(
            Math.max(pos.y, minY),
            Math.max(minY, maxY - 64 * scale),
          );
          if (cx !== pos.x || cy !== pos.y) {
            pos = new PhysicalPosition(cx, cy);
            await win.setPosition(pos);
          }
        }

        const key = `${pos.x}:${pos.y}`;
        if (key === last) stable++;
        else stable = 0;
        last = key;
        // No try cap: a slow drag is legitimate. The poll ends when the
        // position has settled.
        if (stable >= 2) {
          window.clearInterval(iv);
          const scale = await win.scaleFactor();
          await commands.setOverlayPillPosition(pos.x / scale, pos.y / scale);
          void commands.setOverlayPillDragging(false);
        }
      } catch {
        window.clearInterval(iv);
        void commands.setOverlayPillDragging(false);
      }
    }, 250);
  };

  return { onPointerDown };
}

export default RecordingOverlay;
