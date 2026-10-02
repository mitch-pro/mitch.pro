//! Shared application state for mitch-server + the prelude sets ported from
//! server.js (`PUBLIC_API_PATHS`, `CSRF_EXEMPT_PATHS`).
//!
//! Step 6 wires the real auth gate: `check_password_cookie` now runs the full
//! ported flow (mitch_session token -> auth_sessions.json -> validId HMAC ->
//! bans -> passwords.json), plus the rate limiter.

use crate::hosts::SiteConfig;
use crate::static_files::StaticCache;
use std::sync::Arc;

pub struct AppState {
    pub cfg: SiteConfig,
    pub static_cache: StaticCache,
    /// Store handle for the shared SQLite layer.
    pub store: Arc<mitch_lib::data::DataStore>,
    /// ID_SECRET — raw bytes from data/id_secret.key.
    pub id_secret: Vec<u8>,
    /// In-memory rate limiter (rlLog + timing tables).
    pub rate_limiter: mitch_lib::auth::RateLimiter,
    /// `globalCoinMultiplier` (server.js:1159) — admin-settable process state.
    /// f64 stored bit-exact via AtomicU64.
    pub coin_multiplier: std::sync::atomic::AtomicU64,
    /// `casinoEnabled` (server.js:1162) — admin-settable process state.
    pub casino_enabled: std::sync::atomic::AtomicBool,
    /// `casinoRigChance` (server.js:1177) — admin-settable process state.
    pub casino_rig_chance: std::sync::atomic::AtomicU64,
    /// `casinoIntake`/`casinoPayout` — read from data/casino_stats.json at
    /// boot (server.js:1165-1169); casino games update them in Step 12.
    pub casino_intake: std::sync::atomic::AtomicU64,
    pub casino_payout: std::sync::atomic::AtomicU64,
    /// `shadowBans` (server.js:1174) — loaded from data/shadow_bans.json.
    pub shadow_bans: std::sync::RwLock<std::collections::HashSet<String>>,
    /// `proxBlocklist` (server.js:1179).
    pub prox_blocklist: std::sync::RwLock<std::collections::HashSet<String>>,
    /// `featuredGameHref` (server.js:1181).
    pub featured_game_href: std::sync::RwLock<String>,
    /// `picklePresence` (server.js:1260) — norm email -> last-seen ms, as a
    /// JS `Map`: insertion-ordered Vec so the features endpoint's `online`
    /// names list iterates in first-seen order (Map.set keeps position).
    /// Process-local, single-instance assumption preserved from JS.
    #[allow(dead_code)]
    pub pickle_presence: std::sync::Mutex<Vec<(String, i64)>>,
    /// `userPresence` (server.js:1064) — norm email -> {lastSeen, playing}.
    /// The `/ws` heartbeat writer lands with the Step 11 presence work;
    /// friends/list reads it for the online/playing fields.
    #[allow(dead_code)]
    pub user_presence: std::sync::Mutex<std::collections::HashMap<String, UserPresence>>,
    /// `matrixPendingEmailAlerts` (server.js:8497) — key `norm:roomId`.
    /// The delayed-alert scheduler itself lands with the Step 11 DM group;
    /// the cancel path is live so notification reads stay correct.
    pub matrix_pending_email_alerts: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `matrixUserLastSeen` (server.js:851-ish) — norm key -> last-seen ms.
    /// Only the matrix WS layer writes it; that lands with the matrix port,
    /// so the map starts empty and every matrix digest recipient looks
    /// "offline" (the literal JS comparison in the digest worker keeps the
    /// duration-vs-cutoff quirk, see workers_email.rs).
    pub matrix_user_last_seen: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `pendingSecurityCodes` (server.js:2460) — `norm:action` -> code record.
    pub pending_security_codes:
        std::sync::Mutex<std::collections::HashMap<String, PendingSecurityCode>>,
    /// `pendingEmailChanges` (server.js:16049) — change token -> record.
    pub pending_email_changes:
        std::sync::Mutex<std::collections::HashMap<String, PendingEmailChange>>,
    /// `pendingTwoFactor` (server.js:2571) — temp token -> 2FA record.
    pub pending_two_factor: std::sync::Mutex<std::collections::HashMap<String, PendingTwoFactor>>,
    /// `SSO_BRIDGE_TOKENS` (server.js:5761) — bridge token -> SSO record.
    pub sso_bridge_tokens: std::sync::Mutex<std::collections::HashMap<String, SsoBridgeToken>>,
    /// `lastRecaptchaSuccess` (server.js:3597) — sid -> last success ms.
    pub last_recaptcha_success: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `happyHourActive` (server.js:1188) — starts false; the Step 11 worker
    /// flips it inside the school-hour window.
    pub happy_hour_active: std::sync::atomic::AtomicBool,
    /// `computedHappyHour` (server.js:1189) — computed once at boot from the
    /// session log (server.js:25249).
    pub computed_happy_hour: std::sync::atomic::AtomicI64,
    /// `lastDailySummarySentDate` (server.js:4156) — e.g. '2026-05-27'; the
    /// daily-summary scheduler's once-per-date dedupe.
    pub last_daily_summary_sent_date: std::sync::Mutex<String>,
    /// Canvas state (server.js:4304-4556) — in-memory pixel/chunk/lock caches
    /// with the 30s flush and hourly heatmap sweep in `crate::workers`.
    pub canvas: crate::routes::canvas::CanvasState,
    /// `e2eUsers` (server.js:848) — live E2E-DM registrations, keyed by the
    /// canonical nickname (= normalized email; join enforces the equality).
    /// A `Vec` of pairs to preserve the JS object's insertion order (join
    /// overwrites in place; Object.entries iterates first-seen order).
    /// Swept every 60s (entries older than 5 min dropped) in `crate::workers`.
    pub e2e_users: std::sync::Mutex<Vec<(String, E2eUser)>>,
    /// `e2eMessages` (server.js:849) — relayed E2E ciphertexts keyed by the
    /// sorted-pair `e2eKey`, newest-last, 500 cap per conversation.
    pub e2e_messages: std::sync::Mutex<std::collections::HashMap<String, Vec<E2eMessage>>>,
    /// `allSockets` (server.js:858) — the broadcast-socket registry
    /// (`ws.data.isBroadcast`), keyed by connection id. Only the presence/
    /// fan-out subset of `ws.data` is carried (`email` normalized + `sid`).
    pub ws_broadcasts: std::sync::Mutex<std::collections::HashMap<u64, crate::ws::WsClient>>,
    /// Connection id allocator for `ws_broadcasts`.
    pub ws_next_id: std::sync::atomic::AtomicU64,
    /// The fan-out channel standing in for the JS per-socket `ws.send` loop;
    /// every connected broadcast socket subscribes.
    pub ws_tx: tokio::sync::broadcast::Sender<Arc<crate::ws::WsEnvelope>>,
    /// `cvOnline` (server.js:881) — chess-vs online map; `touchUserPresence`
    /// writes under both the raw email and the normalized key. Consumers land
    /// with Step 12 (chess-vs).
    #[allow(dead_code)]
    pub cv_online: std::sync::Mutex<std::collections::HashMap<String, i64>>,

    /// `gamePortalSessions` (server.js:493) — normalized email → active
    /// game-portal reward heartbeat. In-memory only, like JS.
    pub game_portal_sessions: std::sync::Mutex<
        std::collections::HashMap<String, crate::routes::games::GamePortalSession>,
    >,

    /// The six idle/mini-game session maps (server.js:539-615), normalized
    /// email → the JS session object kept verbatim (insertion-ordered Value)
    /// because every endpoint echoes `state: s` straight back. Each has a
    /// `save…Sessions()` write-through to its data/<name>.json document.
    /// Bun loads clicker/typing/logic/richard at boot but NOT piano/piccolo
    /// (server.js:25252-25255) — those two deliberately start empty.
    pub clicker_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    pub typing_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    pub logic_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    pub richard_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    #[allow(dead_code)] // bun never loads these at boot; starts empty
    pub piano_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    #[allow(dead_code)]
    pub piccolo_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,

    /// `logicDictionary` (server.js:542-553) — the 5-letter lowercase words
    /// from data/wordle_dictionary.txt, loaded once at boot (missing file →
    /// empty set, like the JS try/catch).
    pub logic_dictionary: std::sync::Mutex<std::collections::HashSet<String>>,

    /// `bjGames` (server.js:837) — norm email → the active blackjack hand.
    /// In-memory only, like JS: hands die with the process.
    pub bj_games:
        std::sync::Mutex<std::collections::HashMap<String, crate::routes::casino::BjGame>>,
    /// `casinoHistory` (server.js:839) — norm email → the last 25 settled
    /// rounds (newest first). In-memory only.
    pub casino_history: std::sync::Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
    /// `bettingFeed` (server.js:1201) — every settled round, newest first,
    /// 50-cap; read by /api/casino/global-feed and the admin traffic page.
    pub betting_feed: std::sync::Mutex<Vec<serde_json::Value>>,
    /// `jeopardyLobbies` (server.js:888) — gameId → lobby, kept as an
    /// insertion-ordered Vec: /join scans in JS Object.values order. In-memory
    /// only, like JS.
    pub jeopardy_lobbies: std::sync::Mutex<Vec<crate::routes::jeopardy::JeopardyLobby>>,
    /// `jeopardyClueCache` + `jeopardyLastFetch` (server.js:889-892) — lazily
    /// refreshed from data/jeopardy_kids_clean.json on the 24h TTL.
    pub jeopardy_clues: std::sync::Mutex<crate::routes::jeopardy::ClueCache>,
    /// `bsChallenges` (server.js:884) — challenge id → record, insertion-
    /// ordered: the heartbeat's challenge list iterates in JS Object.values
    /// order. In-memory only, like JS.
    pub bs_challenges:
        std::sync::Mutex<indexmap::IndexMap<String, crate::routes::battleship::BsChallenge>>,
    /// `bsGames` (server.js:885) — gameId → game, insertion-ordered (the
    /// heartbeat's activeGames list iterates in Object.values order).
    pub bs_games: std::sync::Mutex<indexmap::IndexMap<String, crate::routes::battleship::BsGame>>,
    /// `bsOnline` (server.js:886) — email → last-seen ms; /online scans in
    /// insertion order.
    pub bs_online: std::sync::Mutex<indexmap::IndexMap<String, i64>>,
    /// `cvGames` (server.js:853) — gameId → raw game JSON, seeded from
    /// data/chess_vs.json at boot (856); insertion order = file order.
    pub cv_games: std::sync::Mutex<indexmap::IndexMap<String, serde_json::Value>>,
    /// `cvChallenges` (server.js:854) — provably empty for the process
    /// lifetime: the /challenge handler never stores (routes/chess_vs.rs).
    /// Kept as state so respond/heartbeat/ping keep their JS shape.
    pub cv_challenges: std::sync::Mutex<indexmap::IndexMap<String, serde_json::Value>>,
    /// `cvChats` (server.js:855) — gameId → messages; in-memory only.
    pub cv_chats: std::sync::Mutex<indexmap::IndexMap<String, serde_json::Value>>,
    /// `activeFreeVms` (server.js:25512) — normalized email → free-VM entry
    /// ({vmid, startedAt, lastActive, ...} as raw JSON). Written by the
    /// Step 13 batch 3 provisioning endpoints; read by the SSH/VNC
    /// authorization slice (`getVmConnectionIpForEmail`).
    pub active_free_vms: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `blooketQueue` (server.js:1263) — FIFO of connected-but-queued
    /// blooket-bot clients, in connect order.
    pub blooket_queue: std::sync::Mutex<Vec<crate::routes::blooket::BlooketQueued>>,
    /// `blooketActive` (server.js:1264) — normalized email → active bot
    /// session, in admission order (the JS Map preserves it).
    pub blooket_active:
        std::sync::Mutex<indexmap::IndexMap<String, crate::routes::blooket::BlooketActive>>,
    /// `blooketPinLocks` (server.js:1265) — game PIN → locking admin email.
    pub blooket_pin_locks: std::sync::Mutex<indexmap::IndexMap<String, String>>,

    /// `vmDesktopSessions` (server.js:27083) — one-time desktop-console
    /// tickets keyed by the random session id. Each record is the JS session
    /// object kept verbatim (actorEmail, sid, authSessionKey, recordId,
    /// ownerEmail, vmid, node, wsUrl, authorization, tlsOptions, expiresAt,
    /// used). TTL 75s (VM_DESKTOP_SESSION_TTL_MS).
    pub vm_desktop_sessions: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `vmDesktopSockets` (server.js:27084) — the live desktop-bridge sockets
    /// (a JS Set of ws objects). Each entry carries the JS `ws.data` subset
    /// the workers/authorization checks read plus a close channel so other
    /// tasks can force `ws.close(1008, …)` the way the JS does.
    pub vm_desktop_sockets:
        std::sync::Mutex<std::collections::HashMap<u64, crate::routes::vm::VmDesktopClient>>,
    /// Connection id allocator for `vm_desktop_sockets`.
    pub vm_desktop_next_id: std::sync::atomic::AtomicU64,
    /// `vmPowerRequests` (server.js:27085) — in-flight create/recreate locks
    /// keyed `create-<norm>` / `recreate-<norm>` (the power gate is
    /// `vm_power_gate`).
    pub vm_power_requests: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `vmPowerGate` (server.js:27086) — one power operation per record.
    pub vm_power_gate: std::sync::Mutex<mitch_lib::vm_security::VmOperationGate>,
    /// `vmLeases` (server.js:27282) — recordId → lease object (raw JSON,
    /// inserted by get_vm_lease exactly like the JS map).
    pub vm_leases: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `vmPagePresence` (server.js:27283) — recordId → { lastSeen } (only
    /// `lastSeen` is ever stored/read).
    pub vm_page_presence: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// Record of when VM shutdown was requested (recordId/key -> timestamp ms)
    /// Used to force-stop VMs that fail to shut off within 10 minutes.
    pub vm_shutdown_initiated: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `lastCapacityNtfy` (server.js:27095) — norm email → last ntfy ms.
    pub last_capacity_ntfy: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `lastAdminUsageNotice` (server.js:27096) — `owner:recordId:op` → ms.
    pub last_admin_usage_notice: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `lastAdminRequestNotice` (server.js:27097) — `owner:recordId:admin` → ms.
    pub last_admin_request_notice: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// `latestAdminBroadcast` (server.js:1334) — polling fallback for broadcasts.
    pub latest_admin_broadcast: std::sync::Mutex<Option<AdminBroadcastEvent>>,
    /// `webauthnChallenges` (server.js:5643) — in-memory WebAuthn challenge store.
    pub webauthn_challenges: mitch_lib::webauthn::ChallengeStore,
}

/// A record in `e2eUsers` (server.js:15384). `priv_key`/`server_pub_hex` are
/// generated per join (the JS CryptoKey / raw-point hex); the JS never reads
/// `priv_key` back today, but the pair is kept for the Step 11 `/ws` relay.
#[allow(dead_code)]
pub struct E2eUser {
    pub pub_key: String,
    pub priv_key: p256::SecretKey,
    pub server_pub_hex: String,
    pub last_seen: i64,
    pub email: String,
}

/// A relayed E2E message in `e2eMessages` (server.js:15795).
#[derive(Clone)]
pub struct E2eMessage {
    pub from: String,
    pub to: String,
    pub data: String,
    pub iv: String,
    pub timestamp: i64,
}

/// A record in `pendingSecurityCodes` (server.js:2482-2486).
pub struct PendingSecurityCode {
    pub code: String,
    pub attempts: u32,
    pub expires: i64,
}

/// A record in `pendingEmailChanges` (server.js:16049).
pub struct PendingEmailChange {
    pub old_norm: String,
    pub new_norm: String,
    pub new_email: String,
    pub code: String,
    pub expires: i64,
    pub attempts: u32,
}

/// A record in `pendingTwoFactor` (server.js:2571).
#[derive(Debug, Clone)]
pub struct PendingTwoFactor {
    pub norm_email: String,
    pub twofa_type: String,
    pub code: Option<String>,
    pub attempts: u32,
    pub expires: i64,
}

/// A record in `SSO_BRIDGE_TOKENS` (server.js:5761).
#[derive(Debug, Clone)]
pub struct SsoBridgeToken {
    pub email: String,
    pub expires: i64,
    pub e2e_private_jwk: Option<serde_json::Value>,
}

/// A record in `userPresence` (server.js:1064, 1112-1116).
#[derive(Clone)]
pub struct UserPresence {
    pub last_seen: i64,
    pub playing: String,
}

/// `PRESENCE_FALLBACK_TTL_MS` (server.js:1065).
pub const PRESENCE_FALLBACK_TTL_MS: i64 = 45_000;

/// `AdminBroadcastEvent` (server.js:1334, 1346-1352).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AdminBroadcastEvent {
    #[serde(rename = "broadcastId")]
    pub broadcast_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub message: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
}

impl AppState {
    pub fn new(cfg: SiteConfig, store: Arc<mitch_lib::data::DataStore>) -> Self {
        let id_secret = mitch_lib::crypto::load_id_secret(&cfg.data_dir).unwrap_or_else(|e| {
            tracing::warn!("id_secret load failed: {e}");
            vec![0u8; 32]
        });
        let casino = store.read_document(
            &cfg.base_dir.join("data/casino_stats.json"),
            serde_json::json!({}),
        );
        let shadow_bans: std::collections::HashSet<String> = store
            .read_document(
                &cfg.base_dir.join("data/shadow_bans.json"),
                serde_json::json!([]),
            )
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let prox_blocklist: std::collections::HashSet<String> = store
            .read_document(
                &cfg.base_dir.join("data/prox_blocklist.json"),
                serde_json::json!([]),
            )
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // `computedHappyHour = getLeastUsedSchoolHour()` at boot
        // (server.js:25249). Uses the same now_ms the boot run would see.
        let computed_happy_hour = mitch_lib::school::get_least_used_school_hour(
            &store,
            &cfg.base_dir.join("data"),
            mitch_lib::school::now_millis(),
        );
        let canvas = crate::routes::canvas::CanvasState::load(&store, &cfg.data_dir.clone());
        // The idle-game session maps are seeded BEFORE the struct literal (the
        // `store` field move happens earlier in the literal than these fields).
        let clicker_map = Self::load_session_map(&store, &cfg.data_dir, "clicker_sessions.json");
        let typing_map = Self::load_session_map(&store, &cfg.data_dir, "typing_sessions.json");
        let logic_map = Self::load_session_map(&store, &cfg.data_dir, "logic_sessions.json");
        let richard_map = Self::load_session_map(&store, &cfg.data_dir, "richard_sessions.json");
        let logic_dictionary = Self::load_logic_dictionary(&cfg.data_dir);
        let cv_games_map = Self::load_cv_games(&store, &cfg.data_dir);
        Self {
            cfg,
            static_cache: StaticCache::new(),
            store,
            id_secret,
            rate_limiter: mitch_lib::auth::RateLimiter::new(),
            coin_multiplier: std::sync::atomic::AtomicU64::new(
                mitch_lib::coins::DEFAULT_COIN_MULTIPLIER.to_bits(),
            ),
            casino_enabled: std::sync::atomic::AtomicBool::new(true),
            casino_rig_chance: std::sync::atomic::AtomicU64::new(0f64.to_bits()),
            casino_intake: std::sync::atomic::AtomicU64::new(
                casino
                    .get("intake")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .to_bits(),
            ),
            casino_payout: std::sync::atomic::AtomicU64::new(
                casino
                    .get("payout")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .to_bits(),
            ),
            shadow_bans: std::sync::RwLock::new(shadow_bans),
            prox_blocklist: std::sync::RwLock::new(prox_blocklist),
            featured_game_href: std::sync::RwLock::new(String::new()),
            pickle_presence: std::sync::Mutex::new(Vec::new()),
            user_presence: std::sync::Mutex::new(std::collections::HashMap::new()),
            matrix_pending_email_alerts: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending_security_codes: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending_email_changes: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending_two_factor: std::sync::Mutex::new(std::collections::HashMap::new()),
            sso_bridge_tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_recaptcha_success: std::sync::Mutex::new(std::collections::HashMap::new()),
            happy_hour_active: std::sync::atomic::AtomicBool::new(false),
            computed_happy_hour: std::sync::atomic::AtomicI64::new(computed_happy_hour),
            last_daily_summary_sent_date: std::sync::Mutex::new(String::new()),
            canvas,
            e2e_users: std::sync::Mutex::new(Vec::new()),
            e2e_messages: std::sync::Mutex::new(std::collections::HashMap::new()),
            ws_broadcasts: std::sync::Mutex::new(std::collections::HashMap::new()),
            ws_next_id: std::sync::atomic::AtomicU64::new(1),
            // 1024-slot queue: per-socket delivery is lossless under normal
            // load; a lagged receiver skips forward like a slow JS client.
            ws_tx: tokio::sync::broadcast::channel(1024).0,
            cv_online: std::sync::Mutex::new(std::collections::HashMap::new()),
            game_portal_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            // Idle/mini-game session maps: clicker/typing/logic/richard are
            // seeded from their documents at boot (server.js:25252-25255);
            // piano/piccolo start empty (the JS never loads them).
            clicker_sessions: std::sync::Mutex::new(clicker_map),
            typing_sessions: std::sync::Mutex::new(typing_map),
            logic_sessions: std::sync::Mutex::new(logic_map),
            richard_sessions: std::sync::Mutex::new(richard_map),
            piano_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            piccolo_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            logic_dictionary: std::sync::Mutex::new(logic_dictionary),
            bj_games: std::sync::Mutex::new(std::collections::HashMap::new()),
            casino_history: std::sync::Mutex::new(std::collections::HashMap::new()),
            betting_feed: std::sync::Mutex::new(Vec::new()),
            jeopardy_lobbies: std::sync::Mutex::new(Vec::new()),
            jeopardy_clues: std::sync::Mutex::new(crate::routes::jeopardy::ClueCache::default()),
            bs_challenges: std::sync::Mutex::new(indexmap::IndexMap::new()),
            bs_games: std::sync::Mutex::new(indexmap::IndexMap::new()),
            bs_online: std::sync::Mutex::new(indexmap::IndexMap::new()),
            cv_games: std::sync::Mutex::new(cv_games_map),
            cv_challenges: std::sync::Mutex::new(indexmap::IndexMap::new()),
            cv_chats: std::sync::Mutex::new(indexmap::IndexMap::new()),
            matrix_user_last_seen: std::sync::Mutex::new(std::collections::HashMap::new()),
            active_free_vms: std::sync::Mutex::new(std::collections::HashMap::new()),
            blooket_queue: std::sync::Mutex::new(Vec::new()),
            blooket_active: std::sync::Mutex::new(indexmap::IndexMap::new()),
            blooket_pin_locks: std::sync::Mutex::new(indexmap::IndexMap::new()),
            vm_desktop_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
            vm_desktop_sockets: std::sync::Mutex::new(std::collections::HashMap::new()),
            vm_desktop_next_id: std::sync::atomic::AtomicU64::new(1),
            vm_power_requests: std::sync::Mutex::new(std::collections::HashMap::new()),
            vm_power_gate: std::sync::Mutex::new(mitch_lib::vm_security::VmOperationGate::new(
                5000,
            )),
            vm_leases: std::sync::Mutex::new(std::collections::HashMap::new()),
            vm_page_presence: std::sync::Mutex::new(std::collections::HashMap::new()),
            vm_shutdown_initiated: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_capacity_ntfy: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_admin_usage_notice: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_admin_request_notice: std::sync::Mutex::new(std::collections::HashMap::new()),
            latest_admin_broadcast: std::sync::Mutex::new(None),
            webauthn_challenges: mitch_lib::webauthn::ChallengeStore::new(),
        }
    }

    /// `Object.assign(cvGames, loadJson(CHESS_VS_FILE, {}))`
    /// (server.js:856) — every document entry becomes an own property,
    /// insertion order = file order.
    fn load_cv_games(
        store: &mitch_lib::data::DataStore,
        data_dir: &std::path::Path,
    ) -> indexmap::IndexMap<String, serde_json::Value> {
        store
            .read_document(&data_dir.join("chess_vs.json"), serde_json::json!({}))
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// `loadXxxSessions()` — `Map(Object.entries(loadJson(FILE, {})))` with a
    /// try/catch → empty map.
    fn load_session_map(
        store: &mitch_lib::data::DataStore,
        data_dir: &std::path::Path,
        name: &str,
    ) -> std::collections::HashMap<String, serde_json::Value> {
        store
            .read_document(&data_dir.join(name), serde_json::json!({}))
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// `loadLogicDictionary()` (server.js:546-548) — readFileSync of
    /// data/wordle_dictionary.txt (a plain file, not a DB document),
    /// 5-letter lowercase words only.
    fn load_logic_dictionary(data_dir: &std::path::Path) -> std::collections::HashSet<String> {
        std::fs::read_to_string(data_dir.join("wordle_dictionary.txt"))
            .unwrap_or_default()
            .split('\n')
            .map(|w| w.trim().to_lowercase())
            .filter(|w| w.chars().count() == 5)
            .collect()
    }

    /// The boot-computed `computedHappyHour` value.
    pub fn happy_hour(&self) -> i64 {
        std::sync::atomic::AtomicI64::load(
            &self.computed_happy_hour,
            std::sync::atomic::Ordering::Relaxed,
        )
    }

    /// `globalCoinMultiplier` as f64.
    pub fn coin_multiplier(&self) -> f64 {
        f64::from_bits(std::sync::atomic::AtomicU64::load(
            &self.coin_multiplier,
            std::sync::atomic::Ordering::Relaxed,
        ))
    }

    pub fn set_coin_multiplier(&self, mult: f64) {
        std::sync::atomic::AtomicU64::store(
            &self.coin_multiplier,
            mult.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// The real `checkPasswordCookie` gate — mitch_session token -> session
    /// store -> validId HMAC -> bans -> passwords.json.
    pub fn check_password_cookie(
        &self,
        headers: &axum::http::HeaderMap,
        sid: Option<&str>,
    ) -> bool {
        let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
        let dev_test_access = mitch_lib::auth::dev_test_access_enabled();
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookies = mitch_lib::auth::get_cookies_from_header_value(
            cookie_header,
            &self.store,
            &self.id_secret,
            node_env_test,
        );
        mitch_lib::auth::check_password_cookie(
            &self.store,
            &self.id_secret,
            &cookies,
            sid,
            node_env_test,
            dev_test_access,
        )
    }

    /// `checkRateLimit` — the per-request rate gate (returns Some when the
    /// request should be rejected with 429).
    pub fn rate_limit_check(
        &self,
        ip: &str,
        id_key: &str,
        endpoint: &str,
    ) -> Option<(u16, &'static str)> {
        mitch_lib::auth::check_rate_limit(&self.rate_limiter, ip, id_key, endpoint)
    }

    /// `softMaintenanceActive` — reads data/soft_maintenance.json each time
    /// (the JS caches it at boot; the file only changes via admin actions).
    pub fn soft_maintenance_active(&self) -> bool {
        std::fs::read_to_string(self.cfg.data_dir.join("soft_maintenance.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.get("active").and_then(|a| a.as_bool()))
            .unwrap_or(false)
    }

    /// `activeAdminBroadcast(now = Date.now())` (server.js:1336-1342).
    pub fn active_admin_broadcast(&self) -> Option<AdminBroadcastEvent> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut guard = self
            .latest_admin_broadcast
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(event) = guard.as_ref() {
            if event.expires_at <= now {
                *guard = None;
                return None;
            }
            return Some(event.clone());
        }
        None
    }

    /// `isRevoked(id)` (server.js:2927-2930).
    pub fn is_revoked_id(&self, sid: &str) -> bool {
        if sid.is_empty() {
            return false;
        }
        let file = self.cfg.data_dir.join("revoked.json");
        let doc = self.store.read_document(&file, serde_json::json!({}));
        doc.as_object().is_some_and(|m| m.contains_key(sid))
    }
}

/// `PUBLIC_API_PATHS` — verbatim.
pub const PUBLIC_API_PATHS: &[&str] = &[
    "/api/webauthn/login/options",
    "/api/webauthn/login/verify",
    "/api/signup",
    "/api/bad-passwords",
    "/api/verify-signup",
    "/api/claim-token",
    "/api/login",
    "/api/dev/test-access",
    "/api/verify-2fa",
    "/api/request-access",
    "/api/newid",
    "/api/pass",
    "/api/games",
    "/api/log-click",
    "/api/newsletter/unsubscribe-direct",
    "/api/token",
    "/api/solve",
    "/api/submit",
    "/api/stats",
    "/api/next",
    "/api/sso/bridge",
    "/api/sso/bridge/handoff",
    "/api/sso/exchange",
    "/api/weather",
    "/api/school-calendar",
    "/api/school-info",
    "/api/bell/override",
    "/api/site-info",
    "/api/guest-session",
    "/api/verify-open",
    "/verify-open.json",
    "/api/backgrounds/list",
    "/api/madlibs",
    "/api/madlibs/random",
    "/api/madlibs/list",
    "/api/matrix/sso-status",
    "/api/matrix/sso-login",
    "/api/matrix/gifs/trending",
    "/api/matrix/gifs/search",
    "/api/matrix/gifs/proxy",
    "/api/matrix/stickers/packs",
    "/api/push/vapid-key",
    "/api/tor/status",
    "/api/tor/newnym",
];

/// `CSRF_EXEMPT_PATHS` (server.js:5705-5728) — verbatim. Team POSTs are NOT
/// exempt (they require X-Mitch-Requested-With like every other /api/ POST).
pub const CSRF_EXEMPT_PATHS: &[&str] = &[
    "/api/sso/exchange",
    "/api/sso/bridge/handoff",
    "/api/dm/attachment/upload",
    "/api/games",
    "/api/premium/email/register",
    "/api/verify-open",
    "/verify-open.json",
    "/api/cache/refresh",
    "/api/admin/cache/refresh",
    "/api/refresh-cache",
    "/api/matrix/sso-login",
    "/api/matrix/sso-status",
    "/api/matrix/moderation/overview",
    "/api/matrix/moderation/set-role",
    "/api/matrix/moderation/kick",
    "/api/matrix/moderation/ban",
    "/api/matrix/moderation/redact",
    "/api/matrix/moderation/slowmode",
    "/api/matrix/moderation/mute-user",
    "/api/matrix/moderation/unmute-user",
    "/api/matrix/moderation/mute-room",
    "/api/matrix/devices/prune-stale",
    "/api/matrix/report-room",
    "/api/matrix/gifs/send",
];
