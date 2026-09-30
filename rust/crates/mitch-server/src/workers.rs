//! Background interval workers (plan Steps 6-13).
//! Replaces the ~20 `setInterval` timers in server.js with tokio tasks on the
//! same schedule. Live so far:
//! - `saveCanvasPixels` 30s flush (server.js:4544-4545)
//! - `canvasHeatmap` hourly 24h sweep (server.js:4788-4793)
//!
//! The presence sweep, weekly digest, daily puzzle, chess-clock warning, DM
//! digest, premium maintenance, nudge, VM purge, happy-hour and DM-prune
//! timers land with their owning steps.

/// Starts every live worker task. Call once from `main` after the state build.
pub fn spawn(state: std::sync::Arc<crate::state::AppState>) {
    // saveCanvasPixels — every 30s (server.js:4545).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tick.tick().await;
                state.canvas.flush_pixels(&state.store, state.data_dir());
            }
        });
    }
    // canvasHeatmap sweep — hourly, dropping entries older than 24h
    // (server.js:4788-4793).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                state.canvas.sweep_heatmap(mitch_lib::school::now_millis());
            }
        });
    }
    // e2eUsers sweeper — every 60s, entries with last_seen older than 5 min
    // dropped (server.js:4044-4049).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                crate::routes::e2e::sweep_e2e_users(&state);
            }
        });
    }

    // userPresence sweeper — every 10s (server.js:1129-1136): drop entries
    // with no broadcast socket and a lastSeen ≥ 45s old, broadcasting
    // `presence_changed` offline for each.
    {
        let state = std::sync::Arc::clone(&state);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                crate::ws::sweep_presence(&state);
            }
        });
    }

    // Step 13 batch 1 — the four email workers (weekly digest, daily puzzle,
    // clock warning, DM digest) plus the DM prune, e2e-attachment cleanup and
    // rlLog sweep timers (server.js:5061-5064, 26352, 25227+, 3477-3483).
    crate::workers_email::spawn(state.clone());
    // Step 13 batch 3 — daily-summary scheduler, premium maintenance and
    // happy hour (server.js:4211-4218, 26783-26784, 26805-26807).
    crate::workers_site::spawn(state.clone());
    // Step 13 batch 4 — VM usage sampling, purge, prune, uptime enforcement, session cleanup.
    crate::workers_vm::spawn(state.clone());

    // Ensure all official Matrix rooms on startup and sync staff roles across all rooms.
    {
        let secret = state.id_secret.clone();
        let store = state.store.clone();
        let data_dir = state.cfg.data_dir.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            for (alias, name, topic) in crate::routes::matrix::OFFICIAL_ROOMS {
                let _ =
                    crate::routes::matrix::ensure_official_room(&secret, alias, name, topic).await;
            }
            crate::routes::matrix::sync_staff_power_levels_to_all_official_rooms(
                &secret, &store, &data_dir,
            )
            .await;
        });
    }
}
