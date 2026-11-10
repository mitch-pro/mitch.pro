//! Live visitor count for rjuhsd.school, protected by proof of work.
//!
//! - `GET  /api/rjuhsd/online` → `{count, challenge}`
//! - `POST /api/rjuhsd/online` `{vid, token, n}` → `{count, challenge}`
//!
//! Only served on rjuhsd.school (and localhost outside production, for local
//! testing); every other host gets a 404. See `docs/rjuhsd-online.md`.
//!
//! Every ping must carry a solved proof of work. The server hands out a
//! stateless challenge token (`exp.bits.nonce.mac`, HMAC-SHA256 over the
//! expiry, difficulty, nonce and the caller's network bucket, keyed from
//! `id_secret`). The client finds a number `n` such that
//! `SHA-256("{token}:{vid}:{n}")` has at least `bits` leading zero bits. The
//! solution is bound to the visitor id, the token, and the caller's IP, and a
//! token works once, so one solve can't be replayed or shared. Difficulty
//! rises with how many visitors one network already has and with global
//! challenge volume, so inflating the count gets more expensive the harder it
//! is pushed.
//!
//! Everything lives in process memory and is never written to disk. A visitor
//! is just a random id and a last-seen time; the count is the number of ids
//! seen in the last [`VISITOR_TTL_MS`].

use crate::errors::json_resp;
use crate::state::AppState;
use axum::http::{header, HeaderMap, Method};
use axum::response::Response;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

/// Leading zero bits required with no load. About 65k hashes on average.
const BASE_BITS: u32 = 16;
/// Extra bits added as one network's live visitor count grows.
const MAX_IP_EXTRA_BITS: u32 = 4;
/// Extra bits added while the whole site is being hit hard.
const PRESSURE_EXTRA_BITS: u32 = 2;
/// Challenges issued per minute, site-wide, before the pressure bits apply.
const PRESSURE_PER_MIN: usize = 3000;
/// Challenges issued per minute to one network (a school NAT shares one IP).
const ISSUE_PER_IP_PER_MIN: usize = 900;
const CHALLENGE_TTL_MS: i64 = 120_000;
const VISITOR_TTL_MS: i64 = 75_000;
/// A visitor id can only be counted once per this interval.
const MIN_PING_GAP_MS: i64 = 20_000;
const MAX_VISITORS: usize = 50_000;
/// Most counted visitors one network can hold at a time.
const MAX_VISITORS_PER_IP: usize = 500;
const PRUNE_EVERY_MS: i64 = 1_000;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, PartialEq, Eq)]
pub enum OnlineError {
    /// Too many challenges requested from one network.
    Limited,
    BadToken,
    Expired,
    BadProof,
    /// The token was already used.
    Replay,
    /// This visitor id pinged again too soon.
    TooSoon,
}

struct Visitor {
    net: String,
    last: i64,
}

#[derive(Default)]
struct Board {
    visitors: HashMap<String, Visitor>,
    per_net: HashMap<String, usize>,
    /// Spent token nonces and when their token expires.
    used: HashMap<String, i64>,
    issued_net: HashMap<String, VecDeque<i64>>,
    issued_all: VecDeque<i64>,
    last_prune: i64,
}

pub struct Challenge {
    pub token: String,
    pub bits: u32,
}

/// Group IPv6 addresses by /64, since one subscriber controls the whole range.
fn net_bucket(ip: &str) -> String {
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        Ok(IpAddr::V4(v4)) => v4.to_string(),
        Err(_) => ip.to_string(),
    }
}

/// `None` only if the key were rejected, which HMAC never does; callers treat
/// it as a failed verification rather than signing with an empty MAC.
fn mac_hex(secret: &[u8], payload: &str) -> Option<String> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).ok()?;
    mac.update(b"rjuhsd-online-pow-v1|");
    mac.update(payload.as_bytes());
    Some(hex(&mac.finalize().into_bytes()[..16]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn leading_zero_bits(digest: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in digest {
        if *byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

/// Valid visitor ids are 16-40 URL-safe characters.
fn valid_vid(vid: &str) -> bool {
    (16..=40).contains(&vid.len())
        && vid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Board {
    fn prune(&mut self, now: i64) {
        if now - self.last_prune < PRUNE_EVERY_MS {
            return;
        }
        self.last_prune = now;
        let expired: Vec<String> = self
            .visitors
            .iter()
            .filter(|(_, v)| now - v.last >= VISITOR_TTL_MS)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(v) = self.visitors.remove(&id) {
                self.release_net(&v.net);
            }
        }
        self.used.retain(|_, exp| *exp > now);
        for log in self.issued_net.values_mut() {
            while log.front().is_some_and(|t| now - t >= 60_000) {
                log.pop_front();
            }
        }
        self.issued_net.retain(|_, log| !log.is_empty());
        while self.issued_all.front().is_some_and(|t| now - t >= 60_000) {
            self.issued_all.pop_front();
        }
    }

    fn release_net(&mut self, net: &str) {
        if let Some(n) = self.per_net.get_mut(net) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.per_net.remove(net);
            }
        }
    }

    pub fn count(&mut self, now: i64) -> usize {
        self.prune(now);
        self.visitors.len()
    }

    /// Difficulty for the next challenge to `net`.
    fn bits_for(&self, net: &str) -> u32 {
        let live = self.per_net.get(net).copied().unwrap_or(0) as u32;
        // +1 bit per doubling past 16 visitors, up to the cap.
        let extra = (32 - (live / 16).leading_zeros()).min(MAX_IP_EXTRA_BITS);
        let pressure = if self.issued_all.len() >= PRESSURE_PER_MIN {
            PRESSURE_EXTRA_BITS
        } else {
            0
        };
        BASE_BITS + extra + pressure
    }

    pub fn issue(&mut self, secret: &[u8], ip: &str, now: i64) -> Result<Challenge, OnlineError> {
        self.prune(now);
        let net = net_bucket(ip);
        let log = self.issued_net.entry(net.clone()).or_default();
        if log.len() >= ISSUE_PER_IP_PER_MIN {
            return Err(OnlineError::Limited);
        }
        log.push_back(now);
        self.issued_all.push_back(now);
        let bits = self.bits_for(&net);
        let mut nonce = [0u8; 12];
        rand::Rng::fill(&mut rand::rng(), &mut nonce);
        let nonce = hex(&nonce);
        let exp = now + CHALLENGE_TTL_MS;
        let body = format!("{exp}.{bits}.{nonce}");
        let mac = mac_hex(secret, &format!("{body}|{net}")).ok_or(OnlineError::BadToken)?;
        Ok(Challenge {
            token: format!("{body}.{mac}"),
            bits,
        })
    }

    /// Verify a solved challenge and count the visitor. The expensive part
    /// for an attacker is producing `n`; everything here is cheap.
    pub fn ping(
        &mut self,
        secret: &[u8],
        ip: &str,
        vid: &str,
        token: &str,
        n: &str,
        now: i64,
    ) -> Result<(), OnlineError> {
        self.prune(now);
        if !valid_vid(vid) || n.is_empty() || n.len() > 16 || !n.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(OnlineError::BadProof);
        }
        let net = net_bucket(ip);
        let parts: Vec<&str> = token.split('.').collect();
        let [exp, bits, nonce, mac] = parts[..] else {
            return Err(OnlineError::BadToken);
        };
        let Some(expected) = mac_hex(secret, &format!("{exp}.{bits}.{nonce}|{net}")) else {
            return Err(OnlineError::BadToken);
        };
        if !constant_time_eq(expected.as_bytes(), mac.as_bytes()) {
            return Err(OnlineError::BadToken);
        }
        let (Ok(exp), Ok(bits)) = (exp.parse::<i64>(), bits.parse::<u32>()) else {
            return Err(OnlineError::BadToken);
        };
        if exp <= now {
            return Err(OnlineError::Expired);
        }
        if self.used.contains_key(nonce) {
            return Err(OnlineError::Replay);
        }
        let digest = Sha256::digest(format!("{token}:{vid}:{n}").as_bytes());
        if leading_zero_bits(&digest) < bits {
            return Err(OnlineError::BadProof);
        }
        // Spend the token only once the work is proven, so a bad guess can't
        // burn a legitimate visitor's challenge.
        self.used.insert(nonce.to_string(), exp);
        if let Some(existing) = self.visitors.get_mut(vid) {
            if now - existing.last < MIN_PING_GAP_MS {
                return Err(OnlineError::TooSoon);
            }
            existing.last = now;
            return Ok(());
        }
        // New visitors past the caps are accepted but not counted.
        if self.visitors.len() >= MAX_VISITORS
            || self.per_net.get(&net).copied().unwrap_or(0) >= MAX_VISITORS_PER_IP
        {
            return Ok(());
        }
        *self.per_net.entry(net.clone()).or_default() += 1;
        self.visitors
            .insert(vid.to_string(), Visitor { net, last: now });
        Ok(())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

static BOARD: OnceLock<Mutex<Board>> = OnceLock::new();

fn board() -> std::sync::MutexGuard<'static, Board> {
    BOARD
        .get_or_init(|| Mutex::new(Board::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// rjuhsd.school only. Localhost is allowed outside production so the page
/// can be tested locally.
fn host_allowed(headers: &HeaderMap) -> bool {
    let host = crate::hosts::request_host(headers).to_lowercase();
    let name = host.split(':').next().unwrap_or("");
    if crate::hosts::is_host(name, "rjuhsd.school") {
        return true;
    }
    std::env::var("NODE_ENV").as_deref() != Ok("production")
        && (name == "localhost" || name == "127.0.0.1")
}

fn respond(code: u16, body: Value) -> Response {
    let mut response = json_resp(code, body);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

fn challenge_json(c: &Challenge) -> Value {
    json!({ "token": c.token, "bits": c.bits })
}

pub fn handle(
    state: &AppState,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<Response> {
    if path != "/api/rjuhsd/online" {
        return None;
    }
    if !host_allowed(headers) {
        return Some(respond(404, json!({"error":"not found"})));
    }
    let ip = crate::handler::get_real_ip(headers, None);
    let now = mitch_lib::school::now_millis();
    let mut board = board();
    if *method == Method::GET {
        return Some(match board.issue(&state.id_secret, &ip, now) {
            Ok(c) => respond(
                200,
                json!({ "count": board.count(now), "challenge": challenge_json(&c) }),
            ),
            Err(_) => respond(429, json!({"error":"too many requests"})),
        });
    }
    if *method != Method::POST {
        return Some(respond(405, json!({"error":"method not allowed"})));
    }
    let Ok(req) = serde_json::from_slice::<Value>(body) else {
        return Some(respond(400, json!({"error":"bad request"})));
    };
    let field = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("");
    // `n` is sent as a string; a JSON number is accepted too.
    let n = match req.get("n") {
        Some(Value::Number(num)) => num.to_string(),
        _ => field("n").to_string(),
    };
    if let Err(err) = board.ping(&state.id_secret, &ip, field("vid"), field("token"), &n, now) {
        let (code, msg) = match err {
            OnlineError::Limited | OnlineError::TooSoon => (429, "slow down"),
            OnlineError::Expired => (400, "challenge expired"),
            OnlineError::Replay => (400, "challenge already used"),
            OnlineError::BadToken | OnlineError::BadProof => (400, "invalid proof"),
        };
        return Some(respond(code, json!({ "error": msg })));
    }
    Some(match board.issue(&state.id_secret, &ip, now) {
        Ok(c) => respond(
            200,
            json!({ "count": board.count(now), "challenge": challenge_json(&c) }),
        ),
        Err(_) => respond(200, json!({ "count": board.count(now) })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-secret";
    const VID: &str = "abcdefghijklmnopqr";

    fn solve(token: &str, vid: &str, bits: u32) -> String {
        for n in 0u64.. {
            let d = Sha256::digest(format!("{token}:{vid}:{n}").as_bytes());
            if leading_zero_bits(&d) >= bits {
                return n.to_string();
            }
        }
        unreachable!()
    }

    fn ping_ok(b: &mut Board, ip: &str, vid: &str, now: i64) -> Result<(), OnlineError> {
        let c = b.issue(SECRET, ip, now).unwrap();
        let n = solve(&c.token, vid, c.bits);
        b.ping(SECRET, ip, vid, &c.token, &n, now)
    }

    #[test]
    fn counts_solved_pings_and_expires_visitors() {
        let mut b = Board::default();
        assert_eq!(b.count(1_000), 0);
        ping_ok(&mut b, "1.2.3.4", VID, 1_000).unwrap();
        ping_ok(&mut b, "1.2.3.5", "zyxwvutsrqponmlkji", 1_000).unwrap();
        assert_eq!(b.count(2_000), 2);
        assert_eq!(b.count(1_000 + VISITOR_TTL_MS + 1), 0);
        assert!(b.per_net.is_empty());
    }

    #[test]
    fn rejects_wrong_proof_without_burning_the_token() {
        let mut b = Board::default();
        let c = b.issue(SECRET, "1.2.3.4", 0).unwrap();
        // Find an n that does NOT satisfy the difficulty.
        let bad = (0u64..)
            .find(|n| {
                leading_zero_bits(&Sha256::digest(format!("{}:{VID}:{n}", c.token).as_bytes()))
                    < c.bits
            })
            .unwrap()
            .to_string();
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, &c.token, &bad, 0),
            Err(OnlineError::BadProof)
        );
        let n = solve(&c.token, VID, c.bits);
        assert_eq!(b.ping(SECRET, "1.2.3.4", VID, &c.token, &n, 0), Ok(()));
    }

    #[test]
    fn token_is_single_use_and_bound_to_vid_and_ip() {
        let mut b = Board::default();
        let c = b.issue(SECRET, "1.2.3.4", 0).unwrap();
        let n = solve(&c.token, VID, c.bits);
        // Another IP can't use this token.
        assert_eq!(
            b.ping(SECRET, "9.9.9.9", VID, &c.token, &n, 0),
            Err(OnlineError::BadToken)
        );
        // Another visitor id can't reuse this solution (overwhelmingly likely).
        let other = "ABCDEFGHIJKLMNOPQR";
        assert!(
            leading_zero_bits(&Sha256::digest(
                format!("{}:{other}:{n}", c.token).as_bytes()
            )) < c.bits
        );
        assert_eq!(b.ping(SECRET, "1.2.3.4", VID, &c.token, &n, 0), Ok(()));
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, &c.token, &n, 0),
            Err(OnlineError::Replay)
        );
    }

    #[test]
    fn rejects_forged_tampered_and_expired_tokens() {
        let mut b = Board::default();
        let c = b.issue(SECRET, "1.2.3.4", 0).unwrap();
        // Lower the claimed difficulty: the MAC no longer matches.
        let forged = c.token.replacen(&format!(".{}.", c.bits), ".1.", 1);
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, &forged, "0", 0),
            Err(OnlineError::BadToken)
        );
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, "garbage", "0", 0),
            Err(OnlineError::BadToken)
        );
        // A token signed with another secret is rejected.
        assert_eq!(
            b.ping(b"other", "1.2.3.4", VID, &c.token, "0", 0),
            Err(OnlineError::BadToken)
        );
        let n = solve(&c.token, VID, c.bits);
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, &c.token, &n, CHALLENGE_TTL_MS + 1),
            Err(OnlineError::Expired)
        );
    }

    #[test]
    fn one_visitor_cannot_ping_faster_than_the_gap() {
        let mut b = Board::default();
        ping_ok(&mut b, "1.2.3.4", VID, 0).unwrap();
        assert_eq!(
            ping_ok(&mut b, "1.2.3.4", VID, MIN_PING_GAP_MS - 1),
            Err(OnlineError::TooSoon)
        );
        ping_ok(&mut b, "1.2.3.4", VID, MIN_PING_GAP_MS).unwrap();
        assert_eq!(b.count(MIN_PING_GAP_MS), 1);
    }

    #[test]
    fn difficulty_rises_with_visitors_from_one_network_and_is_capped() {
        let mut b = Board::default();
        assert_eq!(b.issue(SECRET, "1.2.3.4", 0).unwrap().bits, BASE_BITS);
        // Fill the network directly: solving hundreds of real proofs at this
        // difficulty is too slow for a debug-build unit test.
        let steps = [
            (0, 0),
            (15, 0),
            (16, 1),
            (32, 2),
            (64, 3),
            (128, 4),
            (500, 4),
        ];
        for (live, extra) in steps {
            b.per_net.insert("1.2.3.4".into(), live);
            b.issued_net.clear();
            assert_eq!(
                b.issue(SECRET, "1.2.3.4", 0).unwrap().bits,
                BASE_BITS + extra,
                "{live} live visitors"
            );
        }
        // A different network is unaffected.
        assert_eq!(b.issue(SECRET, "5.6.7.8", 0).unwrap().bits, BASE_BITS);
    }

    #[test]
    fn challenge_issuance_is_rate_limited_per_network() {
        let mut b = Board::default();
        for _ in 0..ISSUE_PER_IP_PER_MIN {
            b.issue(SECRET, "1.2.3.4", 0).unwrap();
        }
        assert!(matches!(
            b.issue(SECRET, "1.2.3.4", 0),
            Err(OnlineError::Limited)
        ));
        assert!(b.issue(SECRET, "1.2.3.5", 0).is_ok());
        assert!(b.issue(SECRET, "1.2.3.4", 61_000).is_ok());
    }

    #[test]
    fn ipv6_addresses_share_a_slash_64_bucket() {
        assert_eq!(
            net_bucket("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
            net_bucket("2001:db8:1:2::1")
        );
        assert_ne!(net_bucket("2001:db8:1:3::1"), net_bucket("2001:db8:1:2::1"));
    }

    #[test]
    fn rejects_malformed_visitor_ids_and_solutions() {
        let mut b = Board::default();
        let c = b.issue(SECRET, "1.2.3.4", 0).unwrap();
        for vid in ["short", "has spaces in the id!!", &"x".repeat(41)] {
            assert_eq!(
                b.ping(SECRET, "1.2.3.4", vid, &c.token, "1", 0),
                Err(OnlineError::BadProof)
            );
        }
        assert_eq!(
            b.ping(SECRET, "1.2.3.4", VID, &c.token, "-1", 0),
            Err(OnlineError::BadProof)
        );
    }
}
