//! Beta tester hub API (`/api/tester/*`) and the sandbox rules it controls,
//! against a real local server and fixture accounts. See docs/tester-hub.md.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::TestServer;
use serde_json::json;

const TESTER: &str = "hubtester@student.rjuhsd.us";
const REGULAR: &str = "hubregular@student.rjuhsd.us";

#[tokio::test(flavor = "multi_thread")]
async fn tester_hub_sandbox_rules() {
    let testers = json!([TESTER]).to_string();
    let s = TestServer::start(&[("testers.json", &testers)], "/api/tester/status", 403).await;
    let tester = s.session_for(TESTER);
    let regular = s.session_for(REGULAR);
    let t = Some(tester.as_str());
    let r = Some(regular.as_str());

    // Access: signed out is refused, regular account 403, listed tester 200.
    let anon = s.call(None, "GET", "/api/tester/status", None).await;
    assert!([401, 403].contains(&anon.status), "signed out is refused");
    assert_eq!(
        s.call(r, "GET", "/api/tester/status", None).await.status,
        403
    );
    let denied = s
        .call(
            r,
            "POST",
            "/api/tester/coins",
            Some(json!({"mode": "unlimited"})),
        )
        .await;
    assert_eq!(denied.status, 403);
    let st = s.call(t, "GET", "/api/tester/status", None).await;
    assert_eq!(st.status, 200);
    assert_eq!(st.json["isPremium"], true);
    assert_eq!(st.json["unlimitedCoins"], true);
    assert_eq!(st.json["coins"].as_f64(), Some(999_999_999.0));
    assert_eq!(st.json["settings"]["unlockAllCosmetics"], false);

    // /api/me reflects the sandbox; the regular account is untouched.
    let wallet = s.call(t, "GET", "/api/profile", None).await;
    assert_eq!(wallet.json["unlimitedCoins"], true);
    assert_eq!(wallet.json["isTester"], true);
    assert_eq!(wallet.json["coins"].as_f64(), Some(999_999_999.0));
    assert_eq!(
        s.call(t, "GET", "/api/me", None).await.json["isPremium"],
        true
    );
    let wallet = s.call(r, "GET", "/api/profile", None).await;
    assert_eq!(wallet.json["unlimitedCoins"], false);
    assert_eq!(wallet.json["coins"].as_f64(), Some(0.0));
    assert_eq!(
        s.call(r, "GET", "/api/me", None).await.json["isPremium"],
        false
    );

    // Coins: custom exact balance, validation, back to unlimited.
    for bad in [
        json!({"mode": "custom", "amount": -5}),
        json!({"mode": "custom", "amount": "x"}),
        json!({"mode": "nope"}),
    ] {
        assert_eq!(
            s.call(t, "POST", "/api/tester/coins", Some(bad))
                .await
                .status,
            400
        );
    }
    let reply = s
        .call(
            t,
            "POST",
            "/api/tester/coins",
            Some(json!({"mode": "custom", "amount": 50})),
        )
        .await;
    assert_eq!(reply.json["ok"], true);
    assert_eq!(reply.json["unlimitedCoins"], false);
    assert_eq!(reply.json["coins"].as_f64(), Some(50.0));
    let wallet = s.call(t, "GET", "/api/profile", None).await;
    assert_eq!(wallet.json["coins"].as_f64(), Some(50.0));
    assert_eq!(wallet.json["unlimitedCoins"], false);
    let reply = s
        .call(
            t,
            "POST",
            "/api/tester/coins",
            Some(json!({"mode": "unlimited"})),
        )
        .await;
    assert_eq!(reply.json["unlimitedCoins"], true);

    // Premium toggle drives the site-wide premium check.
    let reply = s
        .call(
            t,
            "POST",
            "/api/tester/toggle-premium",
            Some(json!({"premium": false})),
        )
        .await;
    assert_eq!(reply.json["isPremium"], false);
    assert_eq!(
        s.call(t, "GET", "/api/me", None).await.json["isPremium"],
        false
    );
    assert_eq!(
        s.call(t, "POST", "/api/tester/toggle-premium", Some(json!({})))
            .await
            .status,
        400
    );
    s.call(
        t,
        "POST",
        "/api/tester/toggle-premium",
        Some(json!({"premium": true})),
    )
    .await;
    assert_eq!(
        s.call(t, "GET", "/api/me", None).await.json["isPremium"],
        true
    );

    // Settings toggles persist independently.
    let reply = s
        .call(
            t,
            "POST",
            "/api/tester/settings",
            Some(json!({"unlockAllCosmetics": true})),
        )
        .await;
    assert_eq!(reply.json["settings"]["unlockAllCosmetics"], true);
    assert_eq!(reply.json["settings"]["bypassCooldowns"], false);
    let reply = s
        .call(
            t,
            "POST",
            "/api/tester/settings",
            Some(json!({"bypassCooldowns": true})),
        )
        .await;
    assert_eq!(reply.json["settings"]["unlockAllCosmetics"], true);
    assert_eq!(reply.json["settings"]["bypassCooldowns"], true);

    // Unlock-all lets the tester equip a cosmetic they never bought.
    let equip = json!({"type": "name_color", "itemId": "neon_purple"});
    let eq = s
        .call(t, "POST", "/api/me/cosmetics/equip", Some(equip.clone()))
        .await;
    assert_eq!(eq.status, 200, "{}", eq.text);
    s.call(
        t,
        "POST",
        "/api/tester/settings",
        Some(json!({"unlockAllCosmetics": false})),
    )
    .await;
    let eq_reg = s
        .call(r, "POST", "/api/me/cosmetics/equip", Some(equip))
        .await;
    assert_eq!(eq_reg.status, 403, "regular accounts still need to own it");

    // Daily claim: second claim blocked, bypass allows it, reset re-opens it.
    let claim = || s.call(t, "POST", "/api/daily-login/claim", Some(json!({})));
    s.call(
        t,
        "POST",
        "/api/tester/settings",
        Some(json!({"bypassCooldowns": false})),
    )
    .await;
    assert_eq!(claim().await.status, 200);
    assert_eq!(claim().await.status, 400);
    s.call(
        t,
        "POST",
        "/api/tester/settings",
        Some(json!({"bypassCooldowns": true})),
    )
    .await;
    assert_eq!(claim().await.status, 200);
    s.call(
        t,
        "POST",
        "/api/tester/settings",
        Some(json!({"bypassCooldowns": false})),
    )
    .await;
    assert_eq!(claim().await.status, 400);
    assert_eq!(
        s.call(t, "POST", "/api/tester/reset-cooldowns", None)
            .await
            .json["ok"],
        true
    );
    assert_eq!(claim().await.status, 200);

    // Sandbox coins can't reach real accounts: marketplace, wagers, gifting.
    let buy = s
        .call(
            t,
            "POST",
            "/api/marketplace/buy",
            Some(json!({"listingId": "x"})),
        )
        .await;
    assert_eq!(buy.status, 403);
    let list = s
        .call(
            t,
            "POST",
            "/api/marketplace/list",
            Some(json!({"type": "text", "price": 1})),
        )
        .await;
    assert_eq!(list.status, 403);
    let wager = s
        .call(
            t,
            "POST",
            "/api/battleship/challenge",
            Some(json!({"to": REGULAR, "bet": 100})),
        )
        .await;
    assert_eq!(wager.status, 400);
    assert!(wager.json["error"]
        .as_str()
        .unwrap_or("")
        .contains("Tester accounts"));
    let gift = s
        .call(
            t,
            "POST",
            "/api/coins/gift-premium",
            Some(json!({"targetEmail": REGULAR})),
        )
        .await;
    assert_eq!(gift.status, 403);

    // Test notification reaches the account.
    let empty = s
        .call(
            t,
            "POST",
            "/api/tester/test-notification",
            Some(json!({"message": ""})),
        )
        .await;
    assert_eq!(empty.status, 400);
    let sent = s
        .call(
            t,
            "POST",
            "/api/tester/test-notification",
            Some(json!({"message": "hello hub"})),
        )
        .await;
    assert_eq!(sent.json["ok"], true);
    let notes = s.call(t, "GET", "/api/me/notifications", None).await;
    assert!(notes.text.contains("hello hub"), "notification delivered");

    // Leaderboard never lists the tester, whatever their balance.
    s.call(
        t,
        "POST",
        "/api/tester/coins",
        Some(json!({"mode": "custom", "amount": 999_999_999})),
    )
    .await;
    let lb = s.call(r, "GET", "/api/leaderboard", None).await;
    assert_eq!(lb.status, 200);
    assert!(!lb.text.contains("hubtester"), "tester not on leaderboard");
}
