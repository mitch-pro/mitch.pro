//! Keeps a repeating problem from filling the log: the first occurrence is
//! logged, then at most one line per `every`, saying how many were skipped.
//! The skipped ones are still available at debug level by the caller.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Seen {
    last: Instant,
    skipped: u64,
}

fn seen() -> &'static Mutex<HashMap<String, Seen>> {
    static SEEN: OnceLock<Mutex<HashMap<String, Seen>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// `Some(skipped)` when `key` should be logged now (`skipped` is how many
/// earlier occurrences were held back since the last line), `None` when it was
/// logged less than `every` ago.
pub fn due(key: &str, every: Duration) -> Option<u64> {
    let mut map = seen().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match map.get_mut(key) {
        Some(s) if now.duration_since(s.last) < every => {
            s.skipped += 1;
            None
        }
        Some(s) => {
            let skipped = std::mem::take(&mut s.skipped);
            s.last = now;
            Some(skipped)
        }
        None => {
            if map.len() > 256 {
                map.clear();
            }
            map.insert(
                key.to_string(),
                Seen {
                    last: now,
                    skipped: 0,
                },
            );
            Some(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_once_then_waits_and_reports_what_it_skipped() {
        let every = Duration::from_millis(60);
        assert_eq!(due("t1", every), Some(0));
        assert_eq!(due("t1", every), None);
        assert_eq!(due("t1", every), None);
        assert_eq!(due("other", every), Some(0), "keys are independent");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(due("t1", every), Some(2));
    }
}
