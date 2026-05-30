# Publisher UI — Design Document

**Status:** Design  
**Companion documents:** [architecture.md](architecture.md) · [narrative-engine-spec.md](narrative-engine-spec.md)

---

## 1. Goals

The publisher binary is a headless background process. The UI is an **operator status window** — not a control surface. Its job is to let a rig operator glance at a monitor and know the system is healthy without opening a terminal.

**Design principles:**
- Lightweight: no web runtime, no Electron, no Node.js — single native binary
- Read-only: the UI displays state; it does not control the engine
- Resilient: UI crash must not kill the publisher pipeline
- Minimal: one window, no settings UI (config lives in `publisher.toml`)

---

## 2. Technology Choice: `egui` + `eframe`

| Option | Binary size | Runtime | Fit |
|---|---|---|---|
| egui / eframe | ~4 MB | None (OpenGL/Vulkan via winit) | ✅ Best fit |
| Tauri | ~10 MB | WebView2 (pre-installed on Win 10/11) | Overkill |
| Windows tray only | — | None | Too minimal |
| Terminal (current) | 0 MB | Node.js | No visibility at a glance |

**`egui`** is an immediate-mode Rust GUI library. It renders the full window each frame from application state. There is no retained widget tree to synchronise. The publisher's internal state (`Arc<Mutex<PublisherStatus>>`) is the single source of truth — the UI just reads it.

```toml
# napi/Cargo.toml (publisher binary)
eframe  = "0.31"
egui    = "0.31"
```

The UI runs on the main thread; the publisher pipeline (iRacing mmap reader + engine + HTTP transport) runs on a background thread. They share state via `Arc<Mutex<PublisherStatus>>`.

---

## 3. Shared State

```rust
/// Written by the publisher pipeline; read by the UI thread.
pub struct PublisherStatus {
    // iRacing connection
    pub iracing_connected: bool,
    pub sub_session_id: Option<i64>,
    pub session_type: Option<String>,       // "Race", "Qualify", etc.
    pub track_name: Option<String>,
    pub session_tick: i64,
    pub session_time_secs: f64,

    // Auth / RC connection
    pub rc_connected: bool,
    pub rc_last_http_status: Option<u16>,
    pub token_expires_at: Option<Instant>,

    // Event counters
    pub events_sent_total: u64,
    pub calls_total: u64,
    pub calls_failed: u64,
    pub last_event_at: Option<Instant>,
    pub last_event_type: Option<String>,

    // Rolling event log (last 50 events)
    pub event_log: VecDeque<EventLogEntry>,
}

pub struct EventLogEntry {
    pub session_time: f64,
    pub event_type: String,
    pub car_number: String,
    pub driver_name: String,
}
```

---

## 4. Window Layout

Single fixed-size window: **480 × 640 px**, non-resizable, always-on-top optional via tray menu.

```
┌─────────────────────────────────────────────────────┐
│  SimCenter Publisher                      [─] [×]   │
├─────────────────────────────────────────────────────┤
│                                                     │
│  ● iRacing      CONNECTED                          │
│    Nürburgring GP · Race · Lap 4 / 25              │
│    SubSession #12345678 · Tick 24000               │
│                                                     │
│  ● Race Control  CONNECTED                         │
│    Token expires in 47 min                         │
│    Last response: 202 · 3 events sent              │
│                                                     │
├─────────────────────────────────────────────────────┤
│  EVENTS  [Total: 142]  [Calls: 67]  [Errors: 0]    │
├─────────────────────────────────────────────────────┤
│  t=342.5  BATTLE_CLOSING    #42 Paul Crofts        │
│  t=341.0  BATTLE_ENGAGED    #42 Paul Crofts        │
│  t=320.0  LAP_COMPLETED     #42 Paul Crofts        │
│  t=290.5  OVERTAKE          #18 Sam Walsh          │
│  t=245.0  LAP_COMPLETED     #42 Paul Crofts        │
│  t=200.0  RACE_GREEN        #42 Paul Crofts        │
│  t=  0.0  SESSION_LOADED    #42 Paul Crofts        │
│  t=  0.0  PUBLISHER_HELLO   #42 Paul Crofts        │
│                                                     │
│  ...                                               │
│                                                     │
├─────────────────────────────────────────────────────┤
│  publisher.toml  ·  d:\simcenter\publisher.toml    │
└─────────────────────────────────────────────────────┘
```

### 4.1 Status indicators

Coloured dot prefix:

| Dot | Colour | Meaning |
|---|---|---|
| ● | Green (`#4caf50`) | Connected / healthy |
| ● | Amber (`#ff9800`) | Degraded — retrying / token near expiry (<5 min) |
| ● | Red (`#f44336`) | Error — not connected / auth failed |
| ● | Grey | Idle — waiting for iRacing to open |

### 4.2 iRacing status block

```
● iRacing      CONNECTED
  <track_name> · <session_type> · Lap <current> / <total>
  SubSession #<sub_session_id> · Tick <session_tick>
```

When iRacing is not running:
```
● iRacing      WAITING FOR SIMULATOR
```

### 4.3 Race Control status block

```
● Race Control  CONNECTED
  Token expires in <N> min
  Last response: 202 · <N> events sent
```

States:

| RC state | Dot | Line 1 | Line 2 |
|---|---|---|---|
| Never connected | Grey | `WAITING` | `—` |
| Auth acquiring | Amber | `AUTHENTICATING` | `—` |
| Auth failed | Red | `AUTH FAILED` | `Check publisher.toml credentials` |
| HTTP error | Amber | `RETRYING (3/3)` | `Last: 500 Internal Server Error` |
| Connected | Green | `CONNECTED` | `Token expires in 47 min` |

### 4.4 Event log

Scrollable list, newest at top. Each row:

```
t=<session_time>  <EVENT_TYPE>    #<car_number> <driver_name>
```

Colour-coded by event family:

| Family | Colour |
|---|---|
| `BATTLE_*` | Amber |
| `OVERTAKE` / `POSITION_LOST` | Green |
| `PIT_*` | Cyan |
| `LAP_COMPLETED` | Light grey |
| `RACE_GREEN` / `RACE_CHECKERED` | White bold |
| `PUBLISHER_*` / `SESSION_*` | Dark grey |

Maximum 50 entries retained in the `VecDeque`. Older entries are dropped.

### 4.5 Footer

Shows the resolved path to the config file loaded at startup. Clicking it opens the file in the default text editor (`std::process::Command::new("notepad").arg(&config_path)`).

---

## 5. System Tray

The window can be minimised to the system tray. Tray icon: a green/amber/red dot matching the worst-case status of the two connections.

Tray right-click menu:
```
SimCenter Publisher (CONNECTED)
─────────────────────────────
Show window
─────────────────────────────
Exit
```

"Exit" sends a `PUBLISHER_GOODBYE` event, flushes the queue, then terminates.

---

## 6. Thread Model

```
main thread          publisher thread         HTTP thread (tokio)
───────────          ──────────────────       ────────────────────
eframe::run()   ←── Arc<Mutex<Status>>   ←── transport flush results
(UI render loop)     ↑                        ↑
                     iRacing mmap reader       azure_identity token
                     engine.process_frame()    ureq POST /v2/ingest
                     event → status update
```

The UI thread calls `ctx.request_repaint_after(Duration::from_millis(500))` — repaint every 500ms. It never blocks.

---

## 7. Binary Entry Point

`src/bin/publisher.rs` grows a `--no-ui` flag:

```
publisher.exe              # default: show UI window
publisher.exe --no-ui      # headless, for running as a Windows Service
```

When `--no-ui` is set, `eframe` is not initialised; the publisher pipeline runs in the main thread and logs to stdout/file only.

---

## 8. Implementation Issues

This design maps to the following GitHub issues:

| Issue | Work |
|---|---|
| #26 `src/bin/publisher.rs` | Main binary + `--no-ui` flag |
| New: publisher UI crate | `egui`/`eframe` window, tray icon, shared state struct |

A new issue should be created for the UI once the RC team confirms the API contract (so the status blocks reflect the final field names).

---

## 9. Open Questions

1. **Always-on-top default?** Rig monitors are often dedicated — always-on-top by default makes sense but should be a tray menu toggle.
2. **Log to file?** The headless `--no-ui` mode should log to `publisher.log` in the same directory. UI mode could show a "Open log file" link in the footer.
3. **Multi-rig view?** Out of scope for v1. Race Control's frontend is the aggregated view.
