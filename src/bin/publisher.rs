//! Publisher binary — connects to iRacing, runs the narrative engine, and
//! streams `PublisherEvent` batches to Race Control via HTTP.
//!
//! # Usage
//!
//! ```powershell
//! publisher.exe [--config <path-to-publisher.toml>] [--no-ui]
//! ```
//!
//! Config can also be supplied entirely via environment variables
//! (see `src/config.rs`). Press Ctrl-C for a clean shutdown.
//! Close the status window or press Ctrl-C when running with the UI.

mod publisher_ui;

fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("[publisher] only supported on Windows (iRacing is Windows-only)");
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    run();
}

#[cfg(target_os = "windows")]
fn run() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use director_narrative_core::{
        config,
        publisher_status::PublisherStatus,
    };

    // ── 1. Load config ────────────────────────────────────────────────────

    let config_path = parse_config_path();
    let no_ui       = std::env::args().any(|a| a == "--no-ui");

    let cfg = match config::load(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[publisher] config error: {e}");
            std::process::exit(1);
        }
    };

    let resolved_config_path = config_path
        .map(|p| p.display().to_string())
        .or_else(|| {
            std::env::current_exe().ok().and_then(|exe| {
                let c = exe.parent()?.join("publisher.toml");
                c.exists().then(|| c.display().to_string())
            })
        })
        .or_else(|| {
            let c = std::path::PathBuf::from("publisher.toml");
            c.exists().then(|| c.display().to_string())
        });

    println!(
        "[publisher] config loaded — api={}",
        cfg.publisher.rc_api_url
    );

    // ── 2. Shared state + shutdown flag ───────────────────────────────────

    let running = Arc::new(AtomicBool::new(true));
    let status  = Arc::new(Mutex::new({
        let mut s = PublisherStatus::default();
        s.config_path = resolved_config_path;
        s
    }));

    // ── 3. Ctrl-C handler ─────────────────────────────────────────────────

    {
        let r = running.clone();
        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .expect("error setting Ctrl-C handler");
    }

    // ── 4. Pipeline thread ────────────────────────────────────────────────

    let pipeline_running = running.clone();
    let pipeline_status  = status.clone();
    let pipeline_cfg     = cfg.clone();

    let pipeline = std::thread::Builder::new()
        .name("publisher-pipeline".into())
        .spawn(move || {
            pipeline_main(pipeline_cfg, pipeline_running, pipeline_status);
        })
        .expect("failed to spawn pipeline thread");

    // ── 5. UI or headless ─────────────────────────────────────────────────

    if no_ui {
        pipeline.join().ok();
    } else {
        if let Err(e) = publisher_ui::run_ui(status, running.clone()) {
            eprintln!("[publisher] UI error: {e}");
        }
        // Window closed — signal pipeline to stop and wait for flush.
        running.store(false, Ordering::SeqCst);
        pipeline.join().ok();
    }

    println!("[publisher] done.");
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn pipeline_main(
    cfg:     director_narrative_core::config::PublisherConfig,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    status:  std::sync::Arc<std::sync::Mutex<director_narrative_core::publisher_status::PublisherStatus>>,
) {
    use std::sync::atomic::Ordering;

    use director_narrative_core::{
        engine::NarrativeEngine,
        lifecycle::LifecyclePublisher,
        publisher_event::build_event,
        publisher_status::EventLogEntry,
        session_info::{parse_sub_session_id, RosterCache, SessionMetadata},
        telemetry_frame::TelemetryFrame,
        transport::PublisherTransport,
    };

    let rig_id = std::env::var("COMPUTERNAME")
        .map(|n| format!("rig-{}", n.to_lowercase()))
        .unwrap_or_else(|_| "rig-unknown".to_string());

    // ── Wait for iRacing ──────────────────────────────────────────────────

    println!("[publisher] waiting for iRacing...");
    let mut reader = match connect_loop(&running) {
        Some(r) => r,
        None => return,
    };

    // ── Initialise components ─────────────────────────────────────────────

    let mut engine      = NarrativeEngine::new(10);
    let mut transport   = PublisherTransport::new(
        &cfg.auth.tenant_id,
        &cfg.auth.client_id,
        &cfg.auth.client_secret,
        &cfg.auth.scope,
        &cfg.publisher.rc_api_url,
        cfg.publisher.batch_interval_ms,
    );
    let mut lifecycle       = LifecyclePublisher::new(env!("CARGO_PKG_VERSION"));
    let mut roster_cache    = RosterCache::new();
    let mut session_meta    = SessionMetadata::default();
    let mut race_session_id = String::from("0");
    let mut sub_session_id: i64 = 0;
    let mut last_frame: Option<TelemetryFrame> = None;

    // ── First frame + PUBLISHER_HELLO ─────────────────────────────────────

    if let Some(frame) = reader.read_frame() {
        let car_info = roster_cache
            .roster()
            .and_then(|r| r.lookup(frame.player_car_idx))
            .map(|c| format!("car=#{} {}", c.car_number, c.driver_name))
            .unwrap_or_else(|| format!("carIdx={}", frame.player_car_idx));
        println!("[publisher] connected — {car_info}");

        let hello = lifecycle.on_activate(frame.lap, frame.session_time);
        let pe    = build_event(&hello, &frame, None, &race_session_id, &rig_id);
        transport.enqueue(pe);

        {
            let mut s = status.lock().unwrap();
            s.iracing_connected     = true;
            s.current_lap           = frame.lap;
            s.session_tick          = frame.session_tick;
            s.session_time_secs     = frame.session_time as f64;
            s.events_enqueued_total += 1;
            let car = roster_cache.roster().and_then(|r| r.lookup(frame.player_car_idx));
            s.push_event_log(EventLogEntry {
                session_time: frame.session_time as f64,
                event_type:   "PUBLISHER_HELLO".into(),
                car_number:   car.map(|c| c.car_number.clone()).unwrap_or_default(),
                driver_name:  car.map(|c| c.driver_name.clone()).unwrap_or_default(),
            });
        }

        last_frame = Some(frame);
    }

    println!(
        "[publisher] publishing at 60 Hz (batch every {}ms)",
        cfg.publisher.batch_interval_ms
    );

    // ── Main loop ─────────────────────────────────────────────────────────

    while running.load(Ordering::SeqCst) {
        if !reader.is_connected() {
            println!("[publisher] iRacing disconnected — reconnecting...");
            {
                let mut s = status.lock().unwrap();
                s.iracing_connected = false;
                s.track_name        = None;
                s.session_type      = None;
                s.session_laps      = None;
            }
            drop(reader);
            reader = match connect_loop(&running) {
                Some(r) => r,
                None    => break,
            };
            engine       = NarrativeEngine::new(10);
            roster_cache = RosterCache::new();
            session_meta = SessionMetadata::default();
            println!("[publisher] reconnected");
            status.lock().unwrap().iracing_connected = true;
            continue;
        }

        match reader.wait_for_frame() {
            Ok(true) => {
                let Some(frame) = reader.read_frame() else { continue };

                // Refresh roster + session metadata when SessionInfo changes.
                if roster_cache.needs_update(frame.session_info_update) {
                    if let Some(yaml) = reader.read_session_info() {
                        if let Some(sid) = parse_sub_session_id(&yaml) {
                            sub_session_id  = sid;
                            race_session_id = sid.to_string();
                        }
                        roster_cache.update(frame.session_info_update, &yaml).ok();
                        session_meta = SessionMetadata::parse(&yaml, frame.session_num as usize);
                        {
                            let mut s = status.lock().unwrap();
                            s.sub_session_id = Some(sub_session_id);
                            s.track_name     = session_meta.track_name.clone();
                            s.session_type   = session_meta.session_type.clone();
                            s.session_laps   = session_meta.session_laps.clone();
                        }
                    }
                }

                let roster = roster_cache.roster();
                let events = engine.process_frame(&frame);

                for event in &events {
                    log_event(event, roster, &frame);
                    let log_entry = make_log_entry(event, &frame, roster);
                    let pe        = build_event(event, &frame, roster, &race_session_id, &rig_id);
                    transport.enqueue(pe);
                    let mut s = status.lock().unwrap();
                    s.events_enqueued_total += 1;
                    s.push_event_log(log_entry);
                }

                // Heartbeat.
                if let Some(hb) = lifecycle.tick(frame.lap, frame.session_time) {
                    let pe = build_event(&hb, &frame, roster, &race_session_id, &rig_id);
                    transport.enqueue(pe);
                    status.lock().unwrap().events_enqueued_total += 1;
                }

                // Frame-level status.
                {
                    let mut s = status.lock().unwrap();
                    s.current_lap       = frame.lap;
                    s.session_tick      = frame.session_tick;
                    s.session_time_secs = frame.session_time as f64;
                    s.token_expires_at  = transport.token_expires_at();
                }

                // Flush.
                match transport.tick_result(
                    frame.session_time as f64,
                    frame.session_tick,
                    sub_session_id,
                ) {
                    Ok(true) => {
                        let mut s = status.lock().unwrap();
                        s.calls_total        += 1;
                        s.rc_connected        = true;
                        s.rc_last_http_status = Some(202);
                        s.token_expires_at    = transport.token_expires_at();
                    }
                    Err(e) => {
                        eprintln!("[transport] flush error: {e}");
                        let mut s = status.lock().unwrap();
                        s.calls_total  += 1;
                        s.calls_failed += 1;
                        s.rc_connected  = false;
                    }
                    Ok(false) => {}
                }

                last_frame = Some(frame);
            }
            Ok(false) => {}
            Err(e)    => eprintln!("[publisher] frame read error: {e}"),
        }
    }

    // ── Clean shutdown ────────────────────────────────────────────────────

    let (bye_lap, bye_t) = last_frame
        .as_ref()
        .map(|f| (f.lap, f.session_time))
        .unwrap_or((0, 0.0));

    let goodbye = lifecycle.on_deactivate(bye_lap, bye_t);
    if let Some(frame) = &last_frame {
        let pe = build_event(
            &goodbye,
            frame,
            roster_cache.roster(),
            &race_session_id,
            &rig_id,
        );
        transport.enqueue(pe);
    }

    println!("[publisher] PUBLISHER_GOODBYE sent — flushing...");
    if let Err(e) = transport.flush(bye_t as f64, 0, sub_session_id) {
        eprintln!("[publisher] flush error: {e}");
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `--config <path>` from command-line arguments.
fn parse_config_path() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| std::path::PathBuf::from(&w[1]))
}

/// Retry loop: block until `IrsdkReader::try_connect()` succeeds.
/// Returns `None` if the shutdown flag is set before a connection is made.
#[cfg(target_os = "windows")]
fn connect_loop(
    running: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<director_narrative_core::irsdk::IrsdkReader> {
    use std::sync::atomic::Ordering;
    use director_narrative_core::irsdk::IrsdkReader;
    loop {
        if !running.load(Ordering::SeqCst) {
            return None;
        }
        match IrsdkReader::try_connect() {
            Ok(r)  => return Some(r),
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

/// Build an `EventLogEntry` for the status panel.
#[cfg(target_os = "windows")]
fn make_log_entry(
    event:  &director_narrative_core::race_event::RaceEvent,
    frame:  &director_narrative_core::telemetry_frame::TelemetryFrame,
    roster: Option<&director_narrative_core::session_info::SessionRoster>,
) -> director_narrative_core::publisher_status::EventLogEntry {
    use director_narrative_core::publisher_status::EventLogEntry;

    let mut event_value = serde_json::to_value(event).unwrap_or_default();
    let event_type = event_value
        .as_object_mut()
        .and_then(|m| m.remove("event_type"))
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();

    let car = roster.and_then(|r| r.lookup(frame.player_car_idx));
    EventLogEntry {
        session_time: frame.session_time as f64,
        event_type,
        car_number:  car.map(|c| c.car_number.clone()).unwrap_or_default(),
        driver_name: car.map(|c| c.driver_name.clone()).unwrap_or_default(),
    }
}

/// Print a concise log line for notable narrative events.
#[cfg(target_os = "windows")]
fn log_event(
    event:  &director_narrative_core::race_event::RaceEvent,
    roster: Option<&director_narrative_core::session_info::SessionRoster>,
    frame:  &director_narrative_core::telemetry_frame::TelemetryFrame,
) {
    use director_narrative_core::race_event::RaceEvent;
    use director_narrative_core::session_info::SessionRoster;

    fn car_num(roster: Option<&SessionRoster>, idx: u8) -> String {
        roster
            .and_then(|r| r.lookup(idx))
            .map(|c| c.car_number.clone())
            .unwrap_or_else(|| idx.to_string())
    }

    match event {
        RaceEvent::RaceGreen { .. }            => println!("[publisher] RACE_GREEN — lap 1 underway"),
        RaceEvent::RaceCheckered { .. }        => println!("[publisher] RACE_CHECKERED"),
        RaceEvent::FlagYellowFullCourse { .. } => println!("[publisher] FLAG_YELLOW_FULL_COURSE"),
        RaceEvent::FlagYellowLocal { .. }      => println!("[publisher] FLAG_YELLOW_LOCAL"),
        RaceEvent::BattleEngaged { car_idx, gap_s, .. } => {
            let player = car_num(roster, frame.player_car_idx);
            let opp    = car_num(roster, *car_idx);
            println!("[publisher] BATTLE_ENGAGED — #{player} vs #{opp}, gap {gap_s:.1}s");
        }
        RaceEvent::BattleClosing { car_idx, closing_rate_sec_per_lap, .. } => {
            let player = car_num(roster, frame.player_car_idx);
            let opp    = car_num(roster, *car_idx);
            println!(
                "[publisher] BATTLE_CLOSING — #{player} vs #{opp}, \
                 closing {closing_rate_sec_per_lap:.1}s/lap"
            );
        }
        RaceEvent::Overtake { position_to, .. } => {
            let player = car_num(roster, frame.player_car_idx);
            println!("[publisher] OVERTAKE — #{player} P{position_to}");
        }
        RaceEvent::OvertakeForLead { .. } => {
            let player = car_num(roster, frame.player_car_idx);
            println!("[publisher] OVERTAKE_FOR_LEAD — #{player} leads");
        }
        _ => {}
    }
}
