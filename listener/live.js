#!/usr/bin/env node
/**
 * Live iRacing narrative event listener — 60 Hz iRacing mmap integration.
 *
 * Connects directly to iRacing's Windows shared-memory file via the Rust
 * native module. A background thread blocks on `IRSDKDataValidEvent` (zero CPU
 * when idle), processes each 60 Hz frame through the NarrativeEngine, and
 * pushes events into this process via NAPI ThreadSafeFunction.
 *
 * Requirements:
 *   - Windows (iRacing runs on Windows only)
 *   - iRacing installed (does not need to be running at startup — the module
 *     will wait until iRacing opens)
 *   - Native module built for Windows:
 *       cd napi && npm run build   (requires cargo + napi-rs CLI)
 *
 * Usage:
 *   node listener/live.js
 *
 * Press Ctrl-C to stop. The background thread shuts down cleanly.
 */

'use strict';

const path = require('path');
const fs   = require('fs');

// ── Load the native module ────────────────────────────────────────────────────

let NarrativeEngine;
const candidates = [
  path.join(__dirname, '..', 'napi', 'index.node'),
  path.join(__dirname, '..', 'target', 'release',
            'director_narrative_core_napi.dll'),
  path.join(__dirname, '..', 'target', 'debug',
            'director_narrative_core_napi.dll'),
];

let loaded = false;
for (const p of candidates) {
  if (fs.existsSync(p)) {
    ({ NarrativeEngine } = require(p));
    loaded = true;
    break;
  }
}

if (!loaded) {
  console.error(
    'ERROR: native module not found.\n' +
    'Build with:\n' +
    '  cd napi && npm run build\n' +
    '  # or for a debug build:\n' +
    '  cargo build -p director-narrative-core-napi\n' +
    '  copy target\\debug\\director_narrative_core_napi.dll napi\\index.node'
  );
  process.exit(1);
}

// ── Verify the platform ───────────────────────────────────────────────────────

if (process.platform !== 'win32') {
  console.error(
    'ERROR: live iRacing mode requires Windows.\n' +
    'For offline testing use:  node listener/index.js data/test_fixture.jsonl'
  );
  process.exit(1);
}

// ── Start live session ────────────────────────────────────────────────────────

// Anchor count defaults to 108 (Nürburgring ~540 s / 5 s = 108 anchors).
// The Rust background thread will recompute this from the first completed lap
// time and rebuild the engine automatically if necessary.
const engine = new NarrativeEngine(108);

console.log('Waiting for iRacing to open...');
console.log('Press Ctrl-C to stop.\n');

engine.startLive((events) => {
  for (const event of events) {
    console.log(JSON.stringify(event, null, 2));
  }
});

// ── Graceful shutdown ─────────────────────────────────────────────────────────

function shutdown() {
  console.log('\nStopping...');
  engine.stopLive();
  process.exit(0);
}

process.on('SIGINT',  shutdown);
process.on('SIGTERM', shutdown);
