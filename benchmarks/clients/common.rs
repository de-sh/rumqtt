use std::{fs::File, io::Write};

use pprof::{protos::Message, ProfilerGuard};
use serde::{Deserialize, Serialize};

pub fn profile(name: &str, guard: ProfilerGuard) {
    if let Ok(report) = guard.report().build() {
        let mut file = File::create(name).unwrap();
        let profile = report.pprof().unwrap();

        let mut content = Vec::new();
        profile.encode(&mut content).unwrap();
        file.write_all(&content).unwrap();

        let svg_name = name.replace(".pb", ".svg");
        let svg_file = File::create(&svg_name).unwrap();
        report.flamegraph(svg_file).unwrap();
    }
}

#[derive(Serialize, Deserialize)]
pub struct Print {
    pub id: String,
    pub messages: usize,
    pub payload_size: usize,
    pub throughput: usize,
}
