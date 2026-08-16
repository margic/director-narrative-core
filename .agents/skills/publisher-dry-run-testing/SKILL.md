---
name: publisher-dry-run-testing
description: Build and exercise the narrative-core Rust publisher (CLI and UI) on a headless Windows VM with no iRacing sim and no steering wheel, and replay its real envelopes into the sandbox director console.
---

# Publisher dry-run testing

How to get real publisher output and real console behavior on a bare Windows VM
with **no iRacing, no steering wheel, and no tailnet access**.

## Build the publisher

`cargo` needs the MSVC toolchain and the VS build env; the
`stable-x86_64-pc-windows-gnu` toolchain fails with a missing `dlltool.exe`.

```bat
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set PATH=C:\Users\Administrator\.cargo\bin;%PATH%
cargo build --bin publisher --bin publisher_ui --features publisher-ui
```

## Telemetry source: the mock mmap

There is no sim, so `scripts/mock_iracing_mmap.py` is the roster/telemetry
source. Always pass **unique** mmap/event names and point the publisher at them
with `SIM_MMAP_NAME` / `SIM_EVENT_NAME`, so parallel runs don't collide:

```
python scripts/mock_iracing_mmap.py --scenario detector-smoke --rate-hz 60 \
    --time-scale 8 --mmap-name "Local\IRSDKMemMapFileName_X" \
    --event-name "Local\IRSDKDataValidEvent_X" --loop
```

Wait for the mock's `start the Rust publisher now` line before launching the
publisher. The `detector-smoke` scenario reports driver *Paul Crofts*
(`userId=341237`, `carIdx=7`), rig `rig-devinbox`, `subSessionId=990001`.

Auth can be fake for `--dry-run` (`PUBLISHER_AUTH_*`, `PUBLISHER_RC_API_URL`
pointing nowhere). Azure token warmup will log `AADSTS900023` and Race Control
shows `DEGRADED` — harmless for dry-run work, so don't chase it.

## The publisher UI needs a software OpenGL driver

`publisher_ui` is egui/glow and dies with `egui_glow requires opengl 2.0+` on a
plain VM. Fix by dropping Mesa3D's `opengl32.dll`, `libgallium_wgl.dll` and
`dxil.dll` next to the exe (`target\debug`) and setting:

```powershell
$env:GALLIUM_DRIVER='llvmpipe'; $env:LIBGL_ALWAYS_SOFTWARE='1'
```

## Driver controls without a wheel

- `publisher.exe --simulate <focus_me|broadcast_toggle>` injects a press with no
  hardware attached. This is the only way to exercise presses here; the Raw
  Input/HID path in `controls_input.rs` cannot be covered without a wheel.
- Bindings live in `controls.toml` **next to the `--config` publisher.toml**
  (`controls_path()`), so use a scratch config dir per run and pre-seed
  `controls.toml` to test the "already bound" and persistence-across-restart UI.
- UI strings worth asserting on: `Driver Controls`, `LISTENING`,
  `press a wheel button…`, `not bound`, and the `Bind` / `Cancel` / `Clear`
  buttons.

## Replaying real envelopes into the sandbox director console

`python -m director_console` (use the repo `.venv`) serves
`http://127.0.0.1:8080`. There is **no synthetic-event endpoint**, and no Event
Hub *send* credentials, so to deliver an event into a running console substitute
only the transport: `RaceEventStream` accepts a `consumer_factory`, so patch
`broadcast_runtime.race_event_stream.RaceEventStream.__init__` to always inject
a fake consumer **before** importing `director_console.api` (the service builds
its stream during lifespan startup). Everything downstream stays real.

Two things that will otherwise waste time:

- The Event Hub body is the **Cosmos document Race Control writes**, not the
  publisher's batch. Take each inner object out of the batch's `events` array
  and stamp the batch's `subSessionId` onto it. The `body` your fake consumer
  hands to `on_event` must be a JSON **string**.
- `DriverControlLedger.REQUEST_TTL_MS` is **20 s** (plus a 30 s per-driver
  cooldown and a 4-per-60 s global cap). Replaying an envelope captured minutes
  earlier is *correctly* dropped as stale and nothing pauses — it looks exactly
  like a bug. Capture a fresh press and deliver it within seconds, and use a new
  `request_id` per press (`request_id` is the dedup key).
- The stream only starts listening once a run is started
  (`RUN BROADCAST AGENT`); before that `listener_active` is false.

Verify pause/resume via `/api/v1/status` (`paused`, `event_stream.connected`,
`events_received`) and via the console Activity list, where the `CYCLE` rows
stop and restart around a `BROADCAST_CONTROL` row.

## What cannot be tested here

MCP servers (OBS, iRacing, comms/Discord) live on tailnet host `dev` and
`tailscale` is not installed, so `/api/v1/status` reports `obs`/`comms`
unreachable and every director cycle in a live console run fails with
`cannot reach OBS MCP`. For director-cycle behavior use the in-memory doubles
pattern from `tests/unit/test_director_focus_requests.py` (fake OBS client,
`_call_iracing_tool`, `_execute_step`, `announce_voice`) and drive
`run_director_cycle` directly — feed it a genuine captured publisher envelope so
only the transport is simulated. Say plainly in reports that no real scene cut,
camera move or voice line was proven.

## Devin Secrets Needed

None for dry-run testing. Real Event Hub delivery would need Race Center /
Event Hub send credentials (`RACECENTER_EVENTHUB_RESOURCE_ID` plus an
Azure identity), and live OBS/sim/Discord work needs tailnet access to host
`dev`.
