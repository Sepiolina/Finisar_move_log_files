//! End-to-end throughput check for many small files.
//!
//! `cargo run --release -p archiver_core --example bench_session -- <files> [months] [dir] [append]`
//!
//! Creates `<files>` small log files (about 2 KiB each) spread over five
//! subfolders and `[months]` months, runs one full session (scan, stage,
//! commit, verify, index, clean up), then checks that every file is in the
//! index and that the source and staging trees are empty. With `[append]`, a
//! second session then adds that many new files to the same months (the
//! daily case: a small batch into a big archive).

use archiver_core::db::ArchivalCandidate;
use archiver_core::schema::open_in_memory;
use archiver_core::session::{run_session, SessionConfig};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let files: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(20_000);
    let months: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1).max(1);
    let root: PathBuf = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("archiver_bench"));
    let append: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(0);
    let _ = fs::remove_dir_all(&root);
    let source = root.join("source");
    let subfolders = ["BURNIN", "ESS", "JTAG", "STM", "Xray"];
    for s in subfolders {
        fs::create_dir_all(source.join(s)).unwrap();
    }

    // Four files per barcode.
    let t = Instant::now();
    let mut candidates = Vec::new();
    let body: String = "2026-10-26 12:00:00 PASS step=42 value=3.14159\n".repeat(44);
    for i in 0..files {
        let bc = i / 4;
        let sub = subfolders[i % subfolders.len()];
        let name = format!("unit_BC{bc:08}_run{i}.log");
        fs::write(source.join(sub).join(name), format!("{i}\n{body}")).unwrap();
        if i % 4 == 0 {
            candidates.push(ArchivalCandidate {
                barcode: format!("BC{bc:08}"),
                last_update: format!("2025-{:02}-15", (bc % months) + 1),
            });
        }
    }
    println!("created {files} files in {:.1?}", t.elapsed());

    let config = SessionConfig {
        source_dir: source.clone(),
        output_dir: root.join("out"),
        staging_dir: root.join("staging"),
        subfolders: subfolders.iter().map(|s| s.to_string()).collect(),
        allowed_extensions: vec!["log".into()],
        lookback_days: 2,
        date_range_override: None,
        workers: 0,
    };
    let mut conn = open_in_memory().unwrap();
    let t = Instant::now();
    let last = std::cell::Cell::new(Instant::now());
    let cb = |msg: String, _: usize, _: usize| {
        if last.get().elapsed().as_secs_f32() > 2.0 || !msg.contains(" of ") {
            println!("  [{:>7.1?}] {msg}", t.elapsed());
            last.set(Instant::now());
        }
    };
    let result = run_session(&mut conn, &config, candidates, "2025-01-01", "2025-12-31", "bench", Some(&cb))
        .expect("session failed");
    let took = t.elapsed();

    let indexed: i64 = conn
        .query_row("SELECT COUNT(*) FROM scanned_log_entry WHERE content_hash IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    let left_in_source: usize = subfolders
        .iter()
        .map(|s| fs::read_dir(source.join(s)).unwrap().count())
        .sum();
    println!(
        "session: {} files moved, {} errors, {} indexed, {} left in source, staging exists: {}",
        result.files_moved,
        result.errors,
        indexed,
        left_in_source,
        config.staging_dir.exists()
    );
    println!("TOTAL {took:.1?}  ({:.0} files/s)", files as f64 / took.as_secs_f64());
    assert_eq!(result.files_moved, files);
    assert_eq!(indexed as usize, files);
    assert_eq!(left_in_source, 0);

    if append > 0 {
        let mut more = Vec::new();
        for i in 0..append {
            let bc = 10_000_000 + i / 4;
            let name = format!("unit_BC{bc:08}_run{i}.log");
            fs::write(source.join(subfolders[i % subfolders.len()]).join(name), format!("{i}\n{body}")).unwrap();
            if i % 4 == 0 {
                more.push(ArchivalCandidate {
                    barcode: format!("BC{bc:08}"),
                    last_update: format!("2025-{:02}-16", (bc % months) + 1),
                });
            }
        }
        let t2 = Instant::now();
        let r = run_session(&mut conn, &config, more, "2025-01-01", "2025-12-31", "bench2", Some(&cb))
            .expect("append session failed");
        println!("APPEND {append} files into existing months: {:.1?} ({} moved)", t2.elapsed(), r.files_moved);
        assert_eq!(r.files_moved, append);
    }
    if std::env::var_os("ARCHIVER_BENCH_KEEP").is_none() {
        let _ = fs::remove_dir_all(&root);
    }
}
