---
name: publisher-dry-run-testing
description: Build and exercise the narrative-core Rust publisher (CLI and UI) on a headless Windows VM with no iRacing sim and no steering wheel, and replay its real envelopes into the sandbox director console.
---

# Publisher dry-run testing

How to get real publisher output and real console behavior on a headless Windows
VM with **no local iRacing and no steering wheel**, covering both the offline
case and the live case where the MCP servers on tailnet host `dev` are reachable.

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

## Live MCP testing against tailnet host `dev`

If `tailscale` is installed and up, the real OBS/sim/comms MCP servers are
reachable and the whole focus path can be driven for real. The console
**defaults to 127.0.0.1**, so it must be launched with:

```
IRACING_MCP_SERVER_URL=http://dev:8765/mcp
OBS_MCP_SERVER_URL=http://dev:8767/mcp
COMMS_MCP_SERVER_URL=http://dev:8768/mcp
```

Only **one** simulator MCP is active on `dev` at a time (iRacing or Le Mans
Ultimate) — discover it via `get_capabilities` rather than assuming.

Things that cost real time here:

- **Instrument the right function.** The director does *not* execute steps via
  `step_executor.default_transport`; it calls its own module-level alias
  `agents.broadcast_director.tools.director._execute_step` (imported from
  `step_executor`) with an `ObsMcpSceneClient`. Wrap **that** to log real
  `obs.set_scene` / `sim.camera_focus` args and results. Also wrap
  `director.announce_voice`: `_focus_ack` runs on a daemon thread and swallows
  the result, so a failed voice line is otherwise completely invisible.
- **Measure dwell externally.** Poll `list_obs_scenes()['current_program_scene']`
  every 250 ms from a separate process. What the cycle *reports* as `dwell_ms`
  and what OBS actually holds on air can differ (see below).
- `ObsMcpSceneClient(server_url)` takes the URL positionally; the methods are
  `list_obs_scenes()`, `set_obs_scene(name)`, `check_connection()` — there is no
  `get_current_program_scene()`.
- **Set OBS routing explicitly** from the SETTINGS tab before testing focus.
  `_build_or_refresh_session_config` bootstraps `director_scene` from whatever
  OBS is currently showing, so an unbound-fallback focus can land on the scene
  already on air and look like nothing happened. Pick a distinct director scene.
- **Session config is in-memory only.** Restarting the console resets
  `config_revision` to 0 and drops routing + driver scene mappings; re-apply them
  through the UI after any restart.
- **Mock publisher vs live roster.** The mock mmap emits Paul Crofts
  `userId=341237` at `carIdx=7`, but scene/driver resolution and the voice ack
  derive the driver from the **live roster entry for that carIdx**. On the `dev`
  iRacing roster `carIdx=7` is a different driver entirely, so the ack names that
  driver. Report the substitution instead of pretending the requester matched.
- Scenes are classified as driver-onboard only when the name contains both
  "driver" and "onboard", and there is no per-driver-named onboard scene on
  `dev` — map the requester's cockpit scene manually in SETTINGS, otherwise focus
  falls back to the director scene (a config gap, not a bug).
- The `[fake-eventhub] delivering ... (<id>)` log line prints the **Cosmos doc
  `id`**, not `payload.request_id`. They differ; don't read that as the harness
  re-stamping envelopes.
- Live MCP servers can be transiently unhealthy (`Unable to reach OBS WebSocket
  at media.local:4455: timed out`, `OBS is not ready to perform the request`,
  and `camera_focus` failing with `Camera did not reach expected carIdx=...
  within 1500ms`). Re-check before concluding a feature is broken, and confirm
  with the rig owner whether the sim was offline. A successful `camera_focus`
  returns `verified: true` with an `observed` block whose `camCarIdx` matches.

### Authored dwell only holds the cut when the step op is `wait.dwell`

`step_executor.execute_step` sleeps **only** for `op == "wait.dwell"` (returning
`{"ok": true, "slept_ms": <step.dwell_ms>}`). A `dwell_ms` authored on any other
op (e.g. `sim.camera_focus`) is metadata that nothing sleeps on, so the scene
flips away almost immediately. This once caused `focus_me` to hold the
requester's onboard scene for **~1.9 s instead of 10 s**; it was fixed by moving
the hold into an explicit `wait.dwell` step. When any pack's on-air time looks
wrong, check whether the pack actually contains a `wait.dwell` step rather than
assuming pacing sampled the value.

How to verify a fixed dwell live, and why a reported number is not enough:

- Measure on-air time **externally** by polling
  `ObsMcpSceneClient(...).list_obs_scenes()["current_program_scene"]` every 250 ms
  in a separate process. Never trust the `dwell_ms` the cycle reports.
- Expect the measured interval to exceed the authored dwell by roughly 1-2 s:
  the `obs.set_scene` round trip, the `sim.camera_focus` call, and the gap before
  the *next* cycle switches scene all sit inside the same on-air window. A 10 s
  authored hold measured ~12 s on air is correct, not over-long.
- The strongest single signal is the `wait.dwell` step result being **exactly**
  `slept_ms: 10000`. A pacing-sampled dwell would land somewhere in the shot
  class window (onboard is 5000-10000 ms) with `dwell_sampled=true`, which is why
  a bare "~10 s measured" is weak evidence on its own.
- `sequence_bind` re-samples dwell for `op in {"sim.camera_focus", "wait.dwell"}`
  but skips sampling when the sequence sets `dwell_fixed: true`, which `focus_me`
  does. Confirm `dwell_fixed` before claiming a hold is fixed-by-design.

### Voice-ack failures surface on stderr, not in the console activity feed

`_announce_focus_ack` checks the `announce_voice` result and emits
`logger.warning("focus request %s voice ack was not played: ...")` when the comms
MCP returns `ok=false`. Nothing in `director_console` calls `logging.basicConfig`
or `dictConfig`, so the root logger is unconfigured and Python's
handler-of-last-resort prints WARNING+ to **stderr**. Redirect the console
process's stderr to its own file (`-RedirectStandardError`) so this warning is
attributable to the product rather than to test instrumentation - it will not
appear in the UI activity feed.

Note `check_voice_runtime` returning `{"ok": true}` does **not** predict the ack
outcome: it has returned ok while `announce_voice` still failed with
`Kokoro request failed: <urlopen error timed out>` (a `dev`-side TTS outage).
When wrapping `announce_voice` for logging, return the real result unchanged so
the product's own `ok=false` branch still runs.

## Fallback when the tailnet is unavailable

If `tailscale` is missing, `/api/v1/status` reports `obs`/`comms` unreachable and
every director cycle fails with `cannot reach OBS MCP`. For director-cycle
behavior use the in-memory doubles pattern from
`tests/unit/test_director_focus_requests.py` (fake OBS client,
`_call_iracing_tool`, `_execute_step`, `announce_voice`) and drive
`run_director_cycle` directly — feed it a genuine captured publisher envelope so
only the transport is simulated. Say plainly in reports that no real scene cut,
camera move or voice line was proven.

## Devin Secrets Needed

`TS_AUTHKEY` for tailnet access to host `dev` (live OBS/sim/comms MCP). None for
pure dry-run testing. Real Event Hub delivery would need Race Center / Event Hub
send credentials (`RACECENTER_EVENTHUB_RESOURCE_ID` plus an Azure identity),
which remain unavailable — the `consumer_factory` seam substitutes only the
transport.
