//! Guards the writer against performance and compression regressions.
//!
//! Run with `mise run bench`.
//!
//! A wall-clock threshold makes a poor gate: it fails on a busy or slow machine
//! and passes on a fast one that has quietly lost half its speed. So the two
//! assertions here are ones that do not depend on how fast the machine is, and
//! throughput is only reported:
//!
//! - **Compression ratio** is deterministic. If an archive grows, the number
//!   moves, whatever it runs on. These are exact bounds.
//! - **Parallel speedup** is measured against this same machine: writing many
//!   entries must take less than writing them one after another would. A writer
//!   that stopped using the other cores fails that anywhere, which is how the
//!   single-threaded write path went unnoticed until a user reported it.
//! - **Throughput** is checked against a floor loose enough that a slow runner
//!   will not trip it. It catches collapses, not drift.

use std::io::Cursor;
use std::time::Instant;

use zesven::write::{WriteOptions, Writer};
use zesven::{ArchivePath, WriteFilter};

/// One scenario's measurement.
struct Measurement {
    name: &'static str,
    wall: f64,
    /// Wall time to write a single entry of the same data, for comparison.
    single_entry_wall: f64,
    entries: usize,
    input: usize,
    output: usize,
}

impl Measurement {
    fn throughput(&self) -> f64 {
        self.input as f64 / self.wall / (1024.0 * 1024.0)
    }

    fn ratio(&self) -> f64 {
        self.output as f64 / self.input as f64
    }

    /// How much faster than compressing the entries one by one.
    ///
    /// Above one means the work really is being spread; the figure is a ratio
    /// of two measurements taken moments apart on the same machine, so it says
    /// nothing about how fast that machine is.
    fn speedup(&self) -> f64 {
        self.single_entry_wall * self.entries as f64 / self.wall
    }
}

/// Data that compresses about as well as source code does.
///
/// Deliberately not a handful of lines repeated: text drawn from a tiny pool
/// is compressed almost entirely by short-range matches, which makes the
/// dictionary size look irrelevant and the ratio look far better than any real
/// corpus would. The identifiers and numbers vary so that matches have to
/// reach different distances, the way they do in a real file.
fn compressible(len: usize) -> Vec<u8> {
    const KEYWORDS: [&str; 12] = [
        "let", "fn", "pub", "struct", "impl", "match", "return", "if", "for", "while", "mut",
        "const",
    ];
    const NAMES: [&str; 12] = [
        "options",
        "encoder",
        "dictionary",
        "buffer",
        "entry",
        "folder",
        "stream",
        "checksum",
        "header",
        "writer",
        "payload",
        "archive",
    ];

    let mut data = Vec::with_capacity(len);
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    while data.len() < len {
        let r = next();
        let line = match r % 5 {
            0 => format!(
                "    {} {}_{} = {};\n",
                KEYWORDS[(r >> 8) as usize % 12],
                NAMES[(r >> 16) as usize % 12],
                r % 1000,
                r % 65536,
            ),
            1 => format!(
                "pub fn {}_{}(&self, {}: &[u8]) -> Result<{}> {{\n",
                NAMES[(r >> 8) as usize % 12],
                r % 100,
                NAMES[(r >> 24) as usize % 12],
                NAMES[(r >> 32) as usize % 12],
            ),
            2 => format!(
                "        // {} the {} before {} it\n",
                KEYWORDS[(r >> 8) as usize % 12],
                NAMES[(r >> 16) as usize % 12],
                NAMES[(r >> 24) as usize % 12],
            ),
            3 => format!("        {}({});\n", NAMES[(r >> 8) as usize % 12], r % 4096),
            _ => "    }\n".to_string(),
        };
        data.extend_from_slice(line.as_bytes());
    }
    data.truncate(len);
    data
}

/// Data that does not compress, like the streams inside a PDF or a JPEG.
fn incompressible(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Writes `count` copies of `data` as separate entries, returning the elapsed
/// time and the packed size.
fn write_entries(options: WriteOptions, data: &[u8], count: usize) -> (f64, usize) {
    let start = Instant::now();

    let mut writer = Writer::create(Cursor::new(Vec::with_capacity(data.len() * count)))
        .unwrap()
        .options(options);
    for i in 0..count {
        writer
            .add_bytes(
                ArchivePath::new(&format!("entry-{i:03}.bin")).unwrap(),
                data,
            )
            .unwrap();
    }
    let (result, _cursor) = writer.finish_into_inner().unwrap();

    (
        start.elapsed().as_secs_f64(),
        result.compressed_size as usize,
    )
}

/// Measures a scenario, plus the single-entry cost it is compared against.
fn measure(name: &'static str, options: WriteOptions, data: &[u8], count: usize) -> Measurement {
    let (single_entry_wall, _) = write_entries(options.clone(), data, 1);
    let (wall, output) = write_entries(options, data, count);

    Measurement {
        name,
        wall,
        single_entry_wall,
        entries: count,
        input: data.len() * count,
        output,
    }
}

/// A scenario's expectations, in terms that do not depend on the machine.
struct Expectation {
    /// The archive must not grow past this fraction of the input.
    max_ratio: f64,
    /// Writing the entries together must beat writing them one by one by this
    /// much. One would mean no parallelism at all.
    min_speedup: f64,
    /// A floor loose enough that only a collapse trips it, in MB/s.
    min_throughput: f64,
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    // Enough entries that every core has something to do, and enough bytes
    // each that per-entry overhead does not dominate.
    let entries = (cores * 2).clamp(8, 32);
    println!("{cores} cores, {entries} entries of 4 MB each\n");

    let source = compressible(4 << 20);
    let opaque = incompressible(4 << 20);

    let cases: Vec<(Measurement, Expectation)> = vec![
        (
            measure(
                "non-solid, level 5, compressible",
                WriteOptions::new(),
                &source,
                entries,
            ),
            Expectation {
                max_ratio: 0.14,
                min_speedup: 1.5,
                min_throughput: 2.0,
            },
        ),
        (
            measure(
                "non-solid, level 1, compressible",
                WriteOptions::new().level(1).unwrap(),
                &source,
                entries,
            ),
            Expectation {
                max_ratio: 0.21,
                min_speedup: 1.5,
                min_throughput: 5.0,
            },
        ),
        (
            measure(
                "non-solid, level 5, incompressible",
                WriteOptions::new(),
                &opaque,
                entries,
            ),
            Expectation {
                max_ratio: 1.01,
                min_speedup: 1.5,
                min_throughput: 1.5,
            },
        ),
        (
            measure(
                "solid, level 5, compressible",
                WriteOptions::new().solid(),
                &source,
                entries,
            ),
            // A solid block sees every entry at once, so it must beat the
            // non-solid ratio rather than merely match it. Its parallelism is
            // chunk-level and so weaker.
            Expectation {
                max_ratio: 0.07,
                min_speedup: 1.1,
                min_throughput: 2.0,
            },
        ),
        (
            measure(
                "non-solid, delta filter, compressible",
                WriteOptions::new().filter(WriteFilter::delta(4)),
                &source,
                entries,
            ),
            Expectation {
                max_ratio: 0.17,
                min_speedup: 1.5,
                min_throughput: 2.0,
            },
        ),
    ];

    println!(
        "{:<40} {:>8} {:>9} {:>9} {:>7}",
        "scenario", "MB/s", "ratio", "speedup", "wall"
    );
    for (m, _) in &cases {
        println!(
            "{:<40} {:>8.1} {:>9.4} {:>8.1}x {:>6.2}s",
            m.name,
            m.throughput(),
            m.ratio(),
            m.speedup(),
            m.wall,
        );
    }
    println!();

    let mut failures = Vec::new();
    for (m, expect) in &cases {
        if m.ratio() > expect.max_ratio {
            failures.push(format!(
                "{}: compressed to {:.4} of the input, worse than the {:.4} allowed",
                m.name,
                m.ratio(),
                expect.max_ratio,
            ));
        }
        // On one or two cores there is no parallelism to lose.
        if cores > 2 && m.speedup() < expect.min_speedup {
            failures.push(format!(
                "{}: only {:.1}x faster than compressing the entries one by one, \
                 expected {:.1}x - the work is not being spread",
                m.name,
                m.speedup(),
                expect.min_speedup,
            ));
        }
        if m.throughput() < expect.min_throughput {
            failures.push(format!(
                "{}: {:.1} MB/s, below the {:.1} MB/s floor",
                m.name,
                m.throughput(),
                expect.min_throughput,
            ));
        }
    }

    if failures.is_empty() {
        println!("all scenarios within expectations");
        return;
    }

    for failure in &failures {
        eprintln!("FAIL: {failure}");
    }
    std::process::exit(1);
}
