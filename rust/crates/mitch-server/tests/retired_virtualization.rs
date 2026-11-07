//! Removed route families stay inaccessible for every role and HTTP method.
//! This exercises the real router and proves they cannot fall through to static
//! assets, generic administrator APIs, or former provisioning handlers.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn removed_vm_families_always_return_not_found() {
    let server = TestServer::start(&[], "/api/vm/status", 404).await;
    let owner = server.session_for("owner@example.test");
    for path in [
        "/vms/",
        "/vms/desktop/",
        "/admin/vms/",
        "/admin/vms/fleet/",
        "/webvm/",
        "/novnc/vnc.html",
        "/vnc/ws",
        "/api/vm/status",
        "/api/vm/my-computer/create",
        "/api/vm/desktop/ws",
        "/api/admin/vms",
        "/api/admin/vm-requests",
        "/api/admin/byo-os-ban",
        "/api/admin/owner-container/start",
    ] {
        for sid in [None, Some(owner.as_str())] {
            for method in ["GET", "POST", "DELETE"] {
                let reply = server.call(sid, method, path, Some(json!({}))).await;
                assert_eq!(reply.status, 404, "{method} {path}: {}", reply.text);
            }
        }
    }
    let db = rusqlite::Connection::open(server.dir.join("data/mitchpro.db")).unwrap();
    let count:i64=db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND (name LIKE 'vm_%' OR name='virtual_machines')",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 0, "Fresh storage must contain no VM models");
}
