//! Route modules — one subsystem per file, mirroring server.js's endpoint
//! groups. No module reaches into another's internals; shared concerns go
//! through `mitch-lib`/`State`.
//!
//! Groups land in plan Steps 7-13, in this order:
//! misc (7) → admin (8) → me + pickle (9) → canvas (10) → dm (11) →
//! games/casino/jeopardy/battleship/chess_vs (12) → team/vm (13).

pub mod admin;
pub mod auth;
pub mod backgrounds;
pub mod battleship;
pub mod blog;
pub mod blooket;
pub mod canvas;
pub mod casino;
pub mod chess_vs;
pub mod daily_login;
pub mod dayboard;
pub mod dm;
pub mod e2e;
pub mod friends;
pub mod games;
pub mod jeopardy;
pub mod livekit;
pub mod madlibs;
pub mod marketplace;
pub mod matrix;
pub mod me;
pub mod members;
pub mod misc;
pub mod pickle;
pub mod proxy;
pub mod push;
pub mod ssh_ws;
pub mod team;
pub mod tor;
pub mod vm;
pub mod webauthn;
