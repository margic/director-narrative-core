#!/usr/bin/env node
/**
 * Smoke test for the napi bindings.
 *
 * Build before running:
 *   cd napi && npm install && npm run build:debug
 *
 * Or using cargo directly (then copy the .so):
 *   cargo build --manifest-path napi/Cargo.toml
 *   cp target/debug/libdirector_narrative_core_napi.so napi/index.node
 *
 * Then:
 *   node napi/smoke_test.js
 */

'use strict';

const path = require('path');
const fs   = require('fs');

// ── Load the native module ────────────────────────────────────────────────────

let NarrativeEngine;
try {
  ({ NarrativeEngine } = require('./index.node'));
} catch (_) {
  // Fallback: look for the debug build artifact produced by `cargo build`
  const candidates = [
    path.join(__dirname, 'index.node'),
    path.join(__dirname, '..', 'target', 'debug',
              'libdirector_narrative_core_napi.so'),
    path.join(__dirname, '..', 'target', 'release',
              'libdirector_narrative_core_napi.so'),
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
      'Build with:  cd napi && npm install && npm run build:debug'
    );
    process.exit(1);
  }
}

// ── Load fixture ──────────────────────────────────────────────────────────────

const fixturePath = path.join(__dirname, '..', 'data', 'test_fixture.jsonl');
if (!fs.existsSync(fixturePath)) {
  console.error('ERROR: fixture missing — run: python3 scripts/synthesize_test_fixture.py');
  process.exit(1);
}

const rawFrames = fs
  .readFileSync(fixturePath, 'utf8')
  .split('\n')
  .filter(Boolean)
  .map(JSON.parse);

// ── camelCase converter (fixture uses snake_case) ─────────────────────────────

function snakeToCamel(obj) {
  const out = {};
  for (const [k, v] of Object.entries(obj)) {
    const key = k.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
    out[key] = Array.isArray(v) ? v : v;
  }
  return out;
}

// ── Compute anchorCount ───────────────────────────────────────────────────────

function computeAnchorCount(frames) {
  const lap1 = frames.find(f => f.lap === 1);
  const lap2 = frames.find(f => f.lap === 2);
  if (!lap1 || !lap2) return 108;
  return Math.max(10, Math.floor((lap2.session_time - lap1.session_time) / 5.0));
}

const anchorCount = computeAnchorCount(rawFrames);
console.log(`anchor_count = ${anchorCount}`);

// ── Run engine ────────────────────────────────────────────────────────────────

const engine = new NarrativeEngine(anchorCount);
const allEvents = [];

for (const raw of rawFrames) {
  const frame = snakeToCamel(raw);
  const events = engine.processFrame(frame);
  allEvents.push(...events);
}

console.log(`Total events: ${allEvents.length}`);
allEvents.forEach(e => console.log(JSON.stringify(e)));

// ── Assertions ────────────────────────────────────────────────────────────────

const pushEvent = allEvents.find(e => e.eventType === 'PUSH');
if (!pushEvent) {
  console.error('FAIL: no PUSH event found in output');
  process.exit(1);
}
console.log('\nPASS: PUSH event found');
console.log('  lap         :', pushEvent.lap);
console.log('  session_time:', pushEvent.sessionTime);
console.log('  context     :', JSON.stringify(pushEvent.narrativeContext));

const attackEvent = allEvents.find(e => e.eventType === 'ATTACK_SETUP');
if (!attackEvent) {
  console.error('FAIL: no ATTACK_SETUP event found in output');
  process.exit(1);
}
console.log('\nPASS: ATTACK_SETUP event found');

const closeEvent = allEvents.find(e => e.eventType === 'CLOSE_APPROACH');
if (!closeEvent) {
  console.error('FAIL: no CLOSE_APPROACH event found in output');
  process.exit(1);
}
console.log('\nPASS: CLOSE_APPROACH event found');
console.log('\nAll smoke tests passed.');
