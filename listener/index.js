#!/usr/bin/env node
/**
 * Narrative event listener — end-to-end demo for director-narrative-core.
 *
 * Reads a JSONL file of telemetry frames (one JSON object per line),
 * feeds each frame into the Rust NarrativeEngine via napi bindings, and
 * logs every emitted RaceEvent to stdout as formatted JSON.
 *
 * Usage:
 *   node listener/index.js <path/to/frames.jsonl>
 *
 * Example:
 *   python3 scripts/synthesize_test_fixture.py
 *   node listener/index.js data/test_fixture.jsonl
 *
 * Build the native module first:
 *   cargo build -p director-narrative-core-napi
 *   cp target/debug/libdirector_narrative_core_napi.so napi/index.node
 *   # or: cd napi && npm install && npm run build:debug
 */

'use strict';

const path = require('path');
const fs   = require('fs');

// ── Load the native module ────────────────────────────────────────────────────

let NarrativeEngine;
const candidates = [
  path.join(__dirname, '..', 'napi', 'index.node'),
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
    'Build with:\n' +
    '  cargo build -p director-narrative-core-napi\n' +
    '  cp target/debug/libdirector_narrative_core_napi.so napi/index.node'
  );
  process.exit(1);
}

// ── Parse arguments ───────────────────────────────────────────────────────────

const jsonlPath = process.argv[2];
if (!jsonlPath) {
  console.error('Usage: node listener/index.js <path/to/frames.jsonl>');
  process.exit(1);
}

const resolvedPath = path.resolve(jsonlPath);
if (!fs.existsSync(resolvedPath)) {
  console.error(`ERROR: file not found: ${resolvedPath}`);
  process.exit(1);
}

// ── Load frames ───────────────────────────────────────────────────────────────

const rawFrames = fs
  .readFileSync(resolvedPath, 'utf8')
  .split('\n')
  .filter(Boolean)
  .map(JSON.parse);

console.log(`Loaded ${rawFrames.length} frames from ${resolvedPath}`);

// ── camelCase converter (JSONL fixture uses snake_case keys) ──────────────────

function snakeToCamel(obj) {
  const out = {};
  for (const [k, v] of Object.entries(obj)) {
    const key = k.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
    out[key] = v;
  }
  return out;
}

// ── Compute anchorCount from first two lap-boundary frames ────────────────────

function computeAnchorCount(frames) {
  const lap1 = frames.find(f => f.lap === 1);
  const lap2 = frames.find(f => f.lap === 2);
  if (!lap1 || !lap2) return 108;
  return Math.max(10, Math.floor((lap2.session_time - lap1.session_time) / 5.0));
}

const anchorCount = computeAnchorCount(rawFrames);
console.log(`anchor_count = ${anchorCount}`);
console.log('');

// ── Stream frames through the engine ─────────────────────────────────────────

const engine = new NarrativeEngine(anchorCount);

for (const raw of rawFrames) {
  const frame  = snakeToCamel(raw);
  const events = engine.processFrame(frame);
  for (const event of events) {
    console.log(JSON.stringify(event, null, 2));
  }
}
