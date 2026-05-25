use director_narrative_core::replay::replay_frames;
use director_narrative_core::telemetry_frame::TelemetryFrame;

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay <path-to-jsonl>");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {path}: {e}"));

    let frames: Vec<TelemetryFrame> = content
        .lines()
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {}: {e}", i + 1))
        })
        .collect();

    let events = replay_frames(&frames);
    for evt in &events {
        println!("{}", serde_json::to_string(evt).expect("serialise event"));
    }
}
