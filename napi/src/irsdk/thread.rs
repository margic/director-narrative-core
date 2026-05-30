//! Background thread that owns the `IrsdkReader` and `NarrativeEngine`.
//!
//! Spawned once by `NarrativeEngine::start_live()`.  The thread blocks on
//! `IrsdkReader::wait_for_frame()` (zero CPU when idle), processes each 60 Hz
//! frame, and pushes non-empty event batches into Node.js via a
//! `ThreadSafeFunction`.
//!
//! **Anchor-count bootstrap:**
//! The engine is constructed with `anchor_count = 108` (Nürburgring default).
//! After the first frame where `LapLastLapTime > 0`, the correct count is
//! derived and the engine is rebuilt if it differs.  Lap 1 is a formation /
//! warm-up lap with no regression data, so no state is lost.
//
// `spawn`, `DEFAULT_ANCHOR_COUNT`, and related items are only called from
// the Windows cfg block in lib.rs. Suppress dead_code on non-Windows.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[cfg(target_os = "windows")]
use std::time::Duration;

use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction};
#[cfg(target_os = "windows")]
use napi::threadsafe_function::ThreadsafeFunctionCallMode;

#[cfg(target_os = "windows")]
use director_narrative_core::engine::NarrativeEngine as CoreEngine;

#[cfg(target_os = "windows")]
use super::super::into_js_event;
use super::super::RaceEvent as JsRaceEvent;

#[cfg(target_os = "windows")]
use super::{IrsdkError, IrsdkReader};

/// Default anchor count used before the first completed lap time is known.
/// Matches a Nürburgring Combined lap (~540 s / 5 s cadence = 108 anchors).
pub(crate) const DEFAULT_ANCHOR_COUNT: usize = 108;

/// Manages the live-session background thread.
pub struct LiveSession {
    shutdown: Arc<AtomicBool>,
    handle:   Option<JoinHandle<()>>,
}

impl LiveSession {
    /// Spawn the background thread.
    ///
    /// `anchor_count` — initial anchor count (use `DEFAULT_ANCHOR_COUNT` if the
    /// first lap time is not yet known).
    /// `tsfn` — NAPI `ThreadSafeFunction` used to push event batches into the
    /// Node.js event loop.
    pub fn spawn(
        anchor_count: usize,
        tsfn: ThreadsafeFunction<Vec<JsRaceEvent>, ErrorStrategy::Fatal>,
    ) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = thread::spawn(move || {
            #[cfg(target_os = "windows")]
            run_live_loop(anchor_count, tsfn, shutdown_clone);

            #[cfg(not(target_os = "windows"))]
            {
                // On non-Windows platforms (Linux CI), the live loop is a no-op.
                // The thread exits immediately.
                let _ = (anchor_count, tsfn, shutdown_clone);
                eprintln!("[irsdk] live mode is only supported on Windows");
            }
        });

        LiveSession { shutdown, handle: Some(handle) }
    }

    /// Signal the thread to stop and wait for it to exit.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Windows live loop ─────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn run_live_loop(
    initial_anchor_count: usize,
    tsfn: ThreadsafeFunction<Vec<JsRaceEvent>, ErrorStrategy::Fatal>,
    shutdown: Arc<AtomicBool>,
) {
    let mut anchor_count = initial_anchor_count;
    let mut engine       = CoreEngine::new(anchor_count);
    let mut bootstrapped = false;   // true once anchor count has been confirmed

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // ── Connect ───────────────────────────────────────────────────────
        let reader = match connect_with_retry(&shutdown) {
            Some(r) => r,
            None    => break,  // shutdown was requested while waiting
        };

        eprintln!("[irsdk] connected — sampling at 60 Hz");

        // ── Read loop ─────────────────────────────────────────────────────
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            match reader.wait_for_frame() {
                Ok(false) => {
                    // Timeout — check connection still alive
                    if !reader.is_connected() {
                        eprintln!("[irsdk] disconnected — reconnecting");
                        break;
                    }
                    continue;
                }
                Err(e) => {
                    eprintln!("[irsdk] wait error: {e}");
                    break;
                }
                Ok(true) => {}
            }

            let frame = match reader.read_frame() {
                Some(f) => f,
                None    => continue,
            };

            // ── Anchor-count bootstrap ─────────────────────────────────
            if !bootstrapped && frame.lap_last_lap_time > 0.0 {
                let computed = (frame.lap_last_lap_time / 5.0)
                    .floor()
                    .max(10.0) as usize;
                if computed != anchor_count {
                    eprintln!(
                        "[irsdk] recomputing anchor_count: {anchor_count} → {computed} \
                         (lap_time={:.1}s)",
                        frame.lap_last_lap_time
                    );
                    anchor_count = computed;
                    engine       = CoreEngine::new(anchor_count);
                }
                bootstrapped = true;
            }

            // ── Process frame ──────────────────────────────────────────
            let events: Vec<JsRaceEvent> = engine
                .process_frame(&frame)
                .into_iter()
                .map(into_js_event)
                .collect();

            if !events.is_empty() {
                tsfn.call(events, ThreadsafeFunctionCallMode::NonBlocking);
            }
        }
    }
}

/// Block until `IrsdkReader::try_connect()` succeeds, polling every second.
/// Returns `None` if the shutdown flag is set before a connection is made.
#[cfg(target_os = "windows")]
fn connect_with_retry(shutdown: &AtomicBool) -> Option<IrsdkReader> {
    eprintln!("[irsdk] waiting for iRacing...");
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return None;
        }
        match IrsdkReader::try_connect() {
            Ok(reader) => return Some(reader),
            Err(IrsdkError::NotRunning) => {
                thread::sleep(Duration::from_secs(1));
            }
            Err(e) => {
                eprintln!("[irsdk] connect error: {e}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}
