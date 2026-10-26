//! A small scoped worker pool (std only), used for the per-file work that
//! dominates a run with hundreds of thousands of small files: renames into
//! staging, verification read-backs and staging cleanup.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How often `tick` is called while workers run.
const TICK_EVERY: Duration = Duration::from_millis(250);

/// Worker threads to use: `requested`, or for `0` the CPU count capped at 8
/// (the work is I/O bound; more threads mostly add disk contention).
pub fn worker_count(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

/// Runs `work` on every item with up to `workers` threads and returns the
/// results in input order. `tick(done)` runs on the calling thread about
/// every 250 ms and once at the end, so a progress callback that is not
/// `Sync` can still report.
pub fn map<T, R>(
    items: &[T],
    workers: usize,
    work: impl Fn(&T) -> R + Sync,
    mut tick: impl FnMut(usize),
) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let workers = workers.clamp(1, items.len().max(1));
    if workers == 1 {
        let mut last = Instant::now();
        let out = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let r = work(item);
                if last.elapsed() >= TICK_EVERY {
                    tick(i + 1);
                    last = Instant::now();
                }
                r
            })
            .collect();
        tick(items.len());
        return out;
    }

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let mut parts: Vec<Vec<(usize, R)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        mine.push((i, work(item)));
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    mine
                })
            })
            .collect();
        let mut last = Instant::now();
        while !handles.iter().all(|h| h.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
            if last.elapsed() >= TICK_EVERY {
                tick(done.load(Ordering::Relaxed));
                last = Instant::now();
            }
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    tick(items.len());
    let mut out: Vec<(usize, R)> = parts.drain(..).flatten().collect();
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_keeps_input_order_and_reports_completion() {
        let items: Vec<u32> = (0..1000).collect();
        let mut last = 0;
        let out = map(&items, 4, |x| x * 2, |d| last = d);
        assert_eq!(out, items.iter().map(|x| x * 2).collect::<Vec<_>>());
        assert_eq!(last, 1000);
        assert!(map(&[] as &[u32], 4, |x| *x, |_| {}).is_empty());
    }
}
