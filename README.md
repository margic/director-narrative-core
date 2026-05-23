# director-narrative-core

`src-telemetry-engine` is a high-performance, Rust-native edge computing module designed for the Sim RaceCenter ecosystem. It ingests high-frequency iRacing telemetry and uses time-series analysis to translate raw physics math (speed, gaps, lap times) into semantic broadcast narratives. By running complex heuristics and state machines on the edge, it feeds the AI Director with the intelligence needed to call undercuts, track long-running battles, and manage race tension with zero performance impact on the simulation.

## Development Environment

This repository includes a Codespaces/devcontainer setup for hybrid Rust + Node.js development:
- Devcontainer config: `.devcontainer/devcontainer.json`
- Rust toolchain support
- Node.js LTS support
- Recommended VS Code extensions for Rust and Node.js workflows

## Prototype Roadmap Backlog

Backlog issue definitions for the SRC Telemetry Engine roadmap are tracked in:
- `BACKLOG_ISSUES.md`


## Phase 1 Interview Prompt

Think about the closest, most tense 30-minute sim race you have recently observed or driven. What is the first specific behavior, or "tell," that indicates to you the dynamic between two cars is changing from a stalemate into an active, aggressive battle?
