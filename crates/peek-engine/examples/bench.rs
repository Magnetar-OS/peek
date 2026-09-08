// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Time the path between pressing space and seeing the file.
//!
//! That delay is what the release profile is tuned for and what the whole
//! design is judged on, so it is worth being able to measure rather than
//! assert. This builds a fixture per previewer, runs `preview::load` over each
//! of them, and reports the distribution.
//!
//! ```sh
//! just bench                  # a table
//! just bench --json           # the same numbers, for a chart
//! just bench --iterations 50
//! ```
//!
//! It deliberately does **not** fail on a threshold. Decode time varies by an
//! order of magnitude between a laptop on battery and a CI runner, so a budget
//! tight enough to catch a regression would fail constantly on the machines it
//! did not have in mind. The numbers are the product; comparing two runs on
//! *one* machine is what finds a regression.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use peek_engine::{Entry, Options, Preview};

/// Times each fixture this many times unless told otherwise.
const DEFAULT_ITERATIONS: usize = 20;

fn main() {
    let mut iterations = DEFAULT_ITERATIONS;
    let mut json = false;

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--iterations" => {
                iterations = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(DEFAULT_ITERATIONS);
            }
            other => {
                eprintln!("usage: bench [--json] [--iterations N]  (got {other:?})");
                std::process::exit(2);
            }
        }
    }

    let fixtures = Fixtures::new();
    let cases = fixtures.build();

    let mut results = Vec::with_capacity(cases.len());
    for (label, path) in &cases {
        let Some(entry) = Entry::load(path) else {
            eprintln!("{label}: fixture could not be read");
            continue;
        };

        // One untimed run first: the syntax set, the GStreamer registry, and
        // the icon theme all load lazily, and charging the first previewer of
        // its kind for that would measure startup rather than decoding.
        let warm = peek_engine::preview::load(&entry, Options::default());
        if matches!(warm, Preview::Failed { .. }) {
            eprintln!("{label}: fixture did not decode; skipping");
            continue;
        }

        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let preview = peek_engine::preview::load(&entry, Options::default());
            samples.push(started.elapsed());
            // Dropped after the clock is read, so deallocating a 30 MB raster
            // is not counted as decode time.
            drop(preview);
        }

        samples.sort_unstable();
        results.push(Measurement {
            label: (*label).to_owned(),
            bytes: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
            best: samples[0],
            median: samples[samples.len() / 2],
            worst: samples[samples.len() - 1],
        });
    }

    if json {
        report_json(&results, iterations);
    } else {
        report_table(&results, iterations);
    }
}

struct Measurement {
    label: String,
    bytes: u64,
    best: Duration,
    median: Duration,
    worst: Duration,
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn report_table(results: &[Measurement], iterations: usize) {
    println!("{iterations} iterations, times in milliseconds\n");
    println!(
        "{:<14} {:>10} {:>10} {:>10} {:>10}",
        "previewer", "size", "best", "median", "worst"
    );
    for result in results {
        println!(
            "{:<14} {:>10} {:>10.2} {:>10.2} {:>10.2}",
            result.label,
            peek_engine::meta::size(result.bytes),
            millis(result.best),
            millis(result.median),
            millis(result.worst)
        );
    }

    let total: f64 = results.iter().map(|result| millis(result.median)).sum();
    println!("\n{:<14} {:>32.2}", "sum of medians", total);
}

fn report_json(results: &[Measurement], iterations: usize) {
    // Written by hand rather than with a serialiser: this is the only place in
    // the workspace that emits JSON, and one format string is smaller than the
    // dependency would be.
    println!("{{");
    println!("  \"iterations\": {iterations},");
    println!("  \"measurements\": [");
    for (index, result) in results.iter().enumerate() {
        let comma = if index + 1 == results.len() { "" } else { "," };
        println!(
            "    {{ \"previewer\": \"{}\", \"bytes\": {}, \"best_ms\": {:.3}, \"median_ms\": {:.3}, \"worst_ms\": {:.3} }}{comma}",
            result.label,
            result.bytes,
            millis(result.best),
            millis(result.median),
            millis(result.worst)
        );
    }
    println!("  ]");
    println!("}}");
}

/// Fixtures, generated and removed with the run.
///
/// Generated rather than committed for the same reason the tests' are: a
/// checked-in JPEG measures how fast that one JPEG decodes, and stops meaning
/// anything the day the encoder changes.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new() -> Self {
        let path = std::env::temp_dir().join("peek-bench");
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the fixture directory");
        Self(path)
    }

    fn build(&self) -> Vec<(&'static str, PathBuf)> {
        let mut cases = Vec::new();

        // A photograph's worth of pixels, which is the case the decode budget
        // was actually chosen for. RGB rather than RGBA: JPEG has no alpha
        // channel, and a real photograph does not either.
        let mut photo = image::RgbImage::new(4000, 3000);
        for (x, y, pixel) in photo.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        let jpeg = self.0.join("photo.jpg");
        photo.save(&jpeg).expect("encode the JPEG");
        cases.push(("image/jpeg", jpeg));

        let png = self.0.join("photo.png");
        photo.save(&png).expect("encode the PNG");
        cases.push(("image/png", png));

        cases.push((
            "vector/svg",
            self.write(
                "diagram.svg",
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300">
                     <rect width="400" height="300" fill="#3584e4"/>
                     <circle cx="200" cy="150" r="90" fill="#e01b24"/>
                     <text x="40" y="40" font-size="24">Peek</text>
                   </svg>"##,
            ),
        ));

        // A source file of the size people actually preview.
        let mut source = String::new();
        for line in 0..2_000 {
            source.push_str(&format!(
                "pub fn generated_{line}(value: u32) -> u32 {{ value.wrapping_mul({line}) }}\n"
            ));
        }
        cases.push(("text/rust", self.write("generated.rs", source.as_bytes())));

        let mut markdown = String::new();
        for section in 0..200 {
            markdown.push_str(&format!(
                "## Section {section}\n\nSome **prose** with `code` and a [link](https://example.com).\n\n- one\n- two\n\n```rust\nfn item_{section}() {{}}\n```\n\n"
            ));
        }
        cases.push(("markdown", self.write("README.md", markdown.as_bytes())));

        cases.push(("archive/zip", self.zip()));

        cases
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("write a fixture");
        path
    }

    fn zip(&self) -> PathBuf {
        let path = self.0.join("bundle.zip");
        let file = std::fs::File::create(&path).expect("create the zip");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for member in 0..500 {
            writer
                .start_file(format!("data/file{member:04}.bin"), options)
                .expect("start");
            writer.write_all(&[b'x'; 512]).expect("write");
        }
        writer.finish().expect("finish the zip");
        path
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
