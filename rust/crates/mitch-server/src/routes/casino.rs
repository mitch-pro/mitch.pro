//! Authenticated MitchCoin casino API; global handler CSRF protection covers bets.
use crate::state::AppState;
use axum::{
    http::{HeaderMap, Method},
    response::Response,
};
use mitch_lib::casino::{self, CasinoError};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Option<Response> {
    if !path.starts_with("/api/casino/") {
        return None;
    }
    let respond = crate::errors::json_resp;
    let Some(email) = crate::presence::authenticated_email(state, headers) else {
        return Some(respond(401, json!({"error":"Sign in to play."})));
    };
    match (method.as_str(), path) {
        ("GET", "/api/casino/state") => Some(match casino::history(&state.store, &email) {
            Ok(rounds) => respond(
                200,
                json!({"balance":mitch_lib::coins::get_coins(&state.store,state.data_dir(),&email),"rounds":rounds}),
            ),
            Err(_) => respond(503, json!({"error":"Casino unavailable. Try again later."})),
        }),
        ("POST", "/api/casino/play") => {
            if state.rate_limit_check(&email, "user", path).is_some() {
                return Some(respond(
                    429,
                    json!({"error":"Please wait before placing another bet."}),
                ));
            }
            let stake = body["stake"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            Some(
                match casino::play(
                    &state.store,
                    &email,
                    body["id"].as_str().unwrap_or(""),
                    body["game"].as_str().unwrap_or(""),
                    body["choice"].as_str().unwrap_or(""),
                    stake,
                ) {
                    Ok(round) => respond(200, json!({"round":round})),
                    Err(CasinoError::Invalid) => respond(
                        400,
                        json!({"error":"Choose a game and a whole-number bet from 1 to 10,000 coins."}),
                    ),
                    Err(CasinoError::Insufficient) => respond(
                        409,
                        json!({"error":"You do not have enough coins for this bet."}),
                    ),
                    Err(CasinoError::Conflict) => respond(
                        409,
                        json!({"error":"This round ID belongs to a different bet. Refresh before playing again."}),
                    ),
                    Err(CasinoError::Unavailable) => respond(
                        503,
                        json!({"error":"Could not settle this round. Retry the same bet to check its result."}),
                    ),
                },
            )
        }
        _ => Some(respond(405, json!({"error":"Method not allowed."}))),
    }
}
