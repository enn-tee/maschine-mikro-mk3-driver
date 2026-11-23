//! Development tool: record raw HID input reports from the Maschine Mikro MK3
//! and measure how long output writes (lights, screen) block.
//!
//! Capture format (text, one report per line):
//!   # comment lines start with '#'
//!   <microseconds since start>\t<report bytes as hex, exactly the bytes read>

use clap::{Parser, Subcommand};
use hidapi::{HidApi, HidDevice, HidResult};
use maschine_library::lights::{Brightness, Lights, PadColors};
use maschine_library::screen::Screen;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{LineWriter, Write};
use std::time::{Duration, Instant};

const VID: u16 = 0x17cc;
const PID: u16 = 0x1700;

#[derive(Parser, Debug)]
#[clap(
    name = "Maschine Mikro MK3 HID dump and timing tool",
    version = env!("CARGO_PKG_VERSION")
)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Record raw input reports with timestamps to a capture file
    Record {
        #[clap(short, long, help = "Capture file to write")]
        out: String,
        #[clap(short, long, default_value_t = 30, help = "How long to record")]
        seconds: u64,
        #[clap(short, long, help = "Free-text note stored in the file header")]
        note: Option<String>,
    },
    /// Measure how long lights and screen writes block
    Timing {
        #[clap(short, long, default_value_t = 100, help = "Writes per measurement")]
        iterations: usize,
    },
}

fn main() -> HidResult<()> {
    let args = Args::parse();
    let api = HidApi::new()?;
    let device = api.open(VID, PID)?;

    match args.command {
        Command::Record { out, seconds, note } => record(&device, &out, seconds, note),
        Command::Timing { iterations } => timing(&device, iterations),
    }
}

fn record(device: &HidDevice, out: &str, seconds: u64, note: Option<String>) -> HidResult<()> {
    // LineWriter so an interrupted run (Ctrl-C) still leaves complete lines on disk.
    let mut file = LineWriter::new(File::create(out).expect("Can't create capture file"));
    let io = "Can't write capture file";

    writeln!(file, "# mikro-hid-capture v1").expect(io);
    writeln!(file, "# vid={VID:04x} pid={PID:04x}").expect(io);
    if let Some(note) = note {
        writeln!(file, "# note: {note}").expect(io);
    }
    writeln!(file, "# columns: micros<TAB>hex bytes").expect(io);

    println!("Recording for {seconds} s to {out} ...");

    let mut buf = [0u8; 64];
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    let mut sizes: BTreeMap<(u8, usize), usize> = BTreeMap::new();
    let start = Instant::now();
    let limit = Duration::from_secs(seconds);

    while start.elapsed() < limit {
        let size = device.read_timeout(&mut buf, 10)?;
        if size == 0 {
            continue;
        }
        let micros = start.elapsed().as_micros();
        let hex: String = buf[..size].iter().map(|b| format!("{b:02x}")).collect();
        writeln!(file, "{micros}\t{hex}").expect(io);
        *counts.entry(buf[0]).or_default() += 1;
        *sizes.entry((buf[0], size)).or_default() += 1;
    }

    println!("Done. Reports by id:");
    for (id, n) in &counts {
        println!("  0x{id:02x}: {n}");
    }
    println!("Reports by id and length:");
    for ((id, size), n) in &sizes {
        println!("  0x{id:02x} len {size}: {n}");
    }
    Ok(())
}

fn timing(device: &HidDevice, iterations: usize) -> HidResult<()> {
    let mut lights = Lights::new();
    let mut screen = Screen::new();

    // Change the content on every write so the device has real work to do.
    let mut light_times = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let b = if i % 2 == 0 {
            Brightness::Normal
        } else {
            Brightness::Off
        };
        lights.set_pad(i % 16, PadColors::Blue, b);
        let t = Instant::now();
        lights.write(device)?;
        light_times.push(t.elapsed());
    }
    lights.reset();
    lights.write(device)?;

    let mut screen_times = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let row = i % 32;
        for col in 0..128 {
            screen.set(row, col, i % 2 == 0);
        }
        let t = Instant::now();
        screen.write(device)?;
        screen_times.push(t.elapsed());
    }
    screen.reset();
    screen.write(device)?;

    // Baseline: the cost of one empty non-blocking input poll.
    let mut buf = [0u8; 64];
    let mut poll_times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        device.read_timeout(&mut buf, 0)?;
        poll_times.push(t.elapsed());
    }

    println!("{iterations} writes each, times in milliseconds");
    report("lights (81 bytes)", &mut light_times);
    report("screen (2 x 265 bytes)", &mut screen_times);
    report("non-blocking read poll", &mut poll_times);
    Ok(())
}

fn report(name: &str, times: &mut [Duration]) {
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let pick = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    println!(
        "  {name:<24} min {:>7.3}  median {:>7.3}  p95 {:>7.3}  max {:>7.3}",
        ms(times[0]),
        ms(pick(0.5)),
        ms(pick(0.95)),
        ms(times[times.len() - 1]),
    );
}
