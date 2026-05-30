//! Publisher binary — connects to iRacing, runs the narrative engine, and
//! streams `PublisherEvent` batches to Race Control via HTTP.
//!
//! # Usage
//!
//! ```powershell
//! publisher.exe [--config <path-to-publisher.toml>]
//! ```
//!
//! Config can also be supplied entirely via environment variables
//! (see `src/config.rs`). Press Ctrl-C for a clean shutdown.

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
    use std::sync::Arc;

    use director_narrative_core::{
        config,
        engine::NarrativeEngine,
        lifecycle::LifecyclePublisher,
        publisher_event::build_event,
        session_info::{parse_sub_session_id, RosterCache},
        telemetry_frame::TelemetryFrame,
        transport::PublisherTransport,
    };

    // ── 1. Load config ────────────────────────────────────────────────────

    let config_path = parse_config_path();
    let cfg = match config::load(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[publisher] config error: {e}");
            std::process::exit(1);
        }
    };

    let rig_id = std::env::var("COMPUTERNAME")
        .map(|n| format!("rig-{}", n.to_lowercase()))
        .unwrap_or_else(|_| "rig-unknown".to_string());

    println!(
        "[publisher] config loaded — rig={rig_id} api={}",
        cfg.publisher.rc_api_url
    );

    // ── 2. Wait for iRacing ───────────────────────────────────────────────

    println!("[publisher] waiting for iRacing...");
    let mut reader = connect_loop();

    // ── 3. Initialise components ──────────────────────────────────────────

    let mut engine = NarrativeEngine::new(10);
    let mut transport = PublisherTransport::new(
        &cfg.auth.tenant_id,
        &cfg.auth.client_id,
        &cfg.auth.client_secret,
        &cfg.auth.scope,
        &cfg.publisher.rc_api_url,
        cfg.publisher.batch_interval_ms,
    );
    let mut lifecycle = LifecyclePublisher::new(env!("CARGO_PKG_VERSION"));
    let mut roster_cache = RosterCache::new();
    let mut race_session_id = String::from("0");
    let mut sub_session_id: i64 = 0;
    let mut last_frame: Option<TelemetryFrame> = None;

    // ── 4. Ctrl-C handler ─────────────────────────────────────────────────

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .expect("error setting Ctrl-C handler");
    }

    // ── 5. First frame + PUBLISHER_HELLO ──────────────────────────────────

    if let Some(frame) = reader.read_frame() {
        let car_info = roster_cache
            .roster()
            .and_then(|r| r.lookup(frame.player_car_idx))
            .map(|c| format!("car=#{} {}", c.car_number, c.driver_name))
            .unwrap_or_else(|| format!("carIdx={}", frame.player_car_idx));
        println!("[publisher] connected — {car_info}");

        let hello = lifecycle.on_activate(frame.lap, frame.session_time);
        let pe = build_event(&hello, &frame, None, &race_session_id, &rig_id);
        transport.enqueue(pe);
        last_frame = Some(frame);
    }

    println!(
        "[publisher] publishing at 60 Hz (batch every {}ms)",
        cfg.publisher.batch_interval_ms
    );

    // ── 6. Main loop ──────────────────────────────────────────────────────

    while running.load(Ordering::SeqCst) {
        // Reconnect if iRacing closed.
        if !reader.is_connected() {
            println!("[publisher] iRacing disconnected — reconnecting...");
            drop(reader);
            reader = connect_loop();
            engine = NarrativeEngine::new(10);
            roster_cache = RosterCache::new();
            println!("[publisher] reconnected");
            continue;
        }

        match reader.wait_for_frame() {
            Ok(true) => {
                let Some(frame) = reader.read_frame() else {
                    continue;
                };

                // Refresh roster when SessionInfo changes.
                if roster_cache.needs_update(frame.session_info_update) {
                    if let Some(yaml) = reader.read_session_info() {
                        if let Some(sid) = parse_sub_session_id(&yaml) {
                            sub_session_id = sid;
                            race_session_id = sid.to_string();
                        }
                        roster_cache.update(frame.session_info_update, &yaml).ok();
                    }
                }

                let roster = roster_cache.roster();

                // Run narrative engine.
                let events = engine.process_frame(&frame);
                for event in &events {
                    log_event(event, roster, &frame);
                    let pe =
                        build_event(event, &frame, roster, &race_session_id, &rig_id);
                    transport.enqueue(pe);
                }

                // Heartbeat (every 30 s).
                if let Some(hb) = lifecycle.tick(frame.lap, frame.session_time) {
                    let pe = build_event(&hb, &frame, roster, &race_session_id, &rig_id);
                    transport.enqueue(pe);
                }

                // Flush batch if interval elapsed.
                transport.tick(
                    frame.session_time as f64,
                    frame.session_tick,
                    sub_session_id,
                );

                last_frame = Some(frame);
            }
            Ok(false) => {
                // 1 s timeout — loop back and check the running flag.
            }
            Err(e) => {
                eprintln!("[publisher] frame read error: {e}");
            }
        }
    }

    // ── 7. Clean shutdown ─────────────────────────────────────────────────

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
    transport
        .flush(bye_t as f64, 0, sub_session_id)
        .unwrap_or_else(|e| eprintln!("[publisher] flush error: {e}"));
    println!("[publisher] done.");
}

/// Parse `--config <path>` from command-line arguments.
fn parse_config_path() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| std::path::PathBuf::from(&w[1]))
}

/// Retry loop: block until `IrsdkReader::try_connect()` succeeds.
#[cfg(target_os = "windows")]
fn connect_loop() -> director_narrative_core::irsdk::IrsdkReader {
    use director_narrative_core::irsdk::IrsdkReader;
    loop {
        match IrsdkReader::try_connect() {
            Ok(r) => return r,
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

/// Print a concise log line for notable narrative events.
#[cfg(target_os = "windows")]
fn log_event(
    event: &director_narrative_core::race_event::RaceEvent,
    roster: Option<&director_narrative_core::session_info::SessionRoster>,
    frame: &director_narrative_core::telemetry_frame::TelemetryFrame,
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
        RaceEvent::RaceGreen { .. } => {
            println!("[publisher] RACE_GREEN — lap 1 underway");
        }
        RaceEvent::RaceCheckered { .. } => {
            println!("[publisher] RACE_CHECKERED");
        }
        RaceEvent::FlagYellowFullCourse { .. } => {
            println!("[publisher] FLAG_YELLOW_FULL_COURSE");
        }
        RaceEvent::FlagYellowLocal { .. } => {
            println!("[publisher] FLAG_YELLOW_LOCAL");
        }
        RaceEvent::BattleEngaged { car_idx, gap_s, .. } => {
            let player = car_num(roster, frame.player_car_idx);
            let opp = car_num(roster, *car_idx);
            println!("[publisher] BATTLE_ENGAGED — #{player} vs #{opp}, gap {gap_s:.1}s");
        }
        RaceEvent::BattleClosing { car_idx, closing_rate_sec_per_lap, .. } => {
            let player = car_num(roster, frame.player_car_idx);
            let opp = car_num(roster, *car_idx);
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
