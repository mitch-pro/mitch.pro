//! Mirrors everything the server prints through `tracing` (the console log)
//! into the `app_logs` table, so the admin "System Logs" viewer shows the same
//! output as the console. Rows older than 30 days are pruned by the store.
//!
//! Wiring: [`init`] runs before the data store exists (the subscriber has to be
//! installed first), so events go into an unbounded channel and [`start_writer`]
//! drains it onto a dedicated thread once the store is open. Writing happens on
//! that thread, never on the caller's, so logging cannot block a request or
//! re-enter the store's lock; the writer itself reports failures with
//! `eprintln!` (not `tracing`) so a failing write can never feed back into the
//! layer.

use std::fmt::Write as _;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use mitch_lib::data::DataStore;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

struct Entry {
    ts: i64,
    level: &'static str,
    category: String,
    message: String,
    details: String,
}

static SENDER: OnceLock<Sender<Entry>> = OnceLock::new();
static RECEIVER: OnceLock<Mutex<Option<Receiver<Entry>>>> = OnceLock::new();

/// Layer to add to the tracing registry. Safe to call once per process.
pub fn init() -> AppLogLayer {
    let (tx, rx) = channel();
    let _ = SENDER.set(tx);
    let _ = RECEIVER.set(Mutex::new(Some(rx)));
    AppLogLayer
}

/// Starts the writer thread and prunes old rows. Call once the store is open.
pub fn start_writer(store: Arc<DataStore>) {
    let Some(rx) = RECEIVER
        .get()
        .and_then(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).take())
    else {
        return;
    };
    if let Err(e) = store.prune_app_logs_sync(now_ms()) {
        eprintln!("app log prune failed: {e}");
    }
    let spawned = std::thread::Builder::new()
        .name("app-log-writer".into())
        .spawn(move || {
            for e in rx {
                if let Err(err) =
                    store.append_app_log_sync(e.ts, e.level, &e.category, e.message, e.details)
                {
                    eprintln!("app log write failed: {err}");
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("could not start app log writer: {e}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `mitch_server::routes::vm` -> `vm`; `mitch_lib::auth` -> `auth`. The store
/// normalizes the rest (lowercase, `[a-z0-9._-]`, 48 chars).
fn category_for(target: &str) -> String {
    let t = target
        .strip_prefix("mitch_server::")
        .or_else(|| target.strip_prefix("mitch_lib::"))
        .or_else(|| target.strip_prefix("mitch_mail::"))
        .unwrap_or(target);
    t.strip_prefix("routes::").unwrap_or(t).replace("::", ".")
}

#[derive(Default)]
struct Fields {
    message: String,
    details: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            if !self.details.is_empty() {
                self.details.push(' ');
            }
            let _ = write!(self.details, "{}={value:?}", field.name());
        }
    }
}

pub struct AppLogLayer;

impl<S: Subscriber> Layer<S> for AppLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let Some(tx) = SENDER.get() else { return };
        let meta = event.metadata();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let level = match *meta.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            Level::INFO => "info",
            _ => "debug",
        };
        let _ = tx.send(Entry {
            ts: now_ms(),
            level,
            category: category_for(meta.target()),
            message: fields.message,
            details: fields.details,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::category_for;

    #[test]
    fn categories_drop_crate_and_routes_prefix() {
        assert_eq!(category_for("mitch_server::routes::vm"), "vm");
        assert_eq!(category_for("mitch_server::workers_vm"), "workers_vm");
        assert_eq!(category_for("mitch_lib::auth"), "auth");
        assert_eq!(category_for("hyper::proto"), "hyper.proto");
    }
}
