# Who can open a VM

"Open" means anything that reaches the computer itself: the desktop console,
start/restart/shut down/force stop, status and heartbeats
(`/api/vm/computers/:id/...`). The one check is
`vm_record_allowed_for_actor` in `rust/crates/mitch-server/src/routes/vm.rs`.

| Who | Access |
| --- | --- |
| The computer's owner | Always |
| The site owner (`auth::is_owner_email`) | Always. They can reach every VM through Proxmox anyway; their use is still recorded in the VM audit log |
| Site admins and co-owners | Only while the computer's owner has allowed admin access |
| Moderators, other members | Never |

Owner-locked computers (`auth::is_vm_owner_locked`: a site owner's own VM and
`OWNER_TEST_ACCOUNT_EMAIL`) never honor a grant; only that account and a site
owner can open them. The grant endpoint refuses them, and the access check
ignores any grant row for them.

## Allowing admins

On `/vms/` the computer's owner sees an **Admin access** section. "Allow
admins" shows a confirmation that spells out what it means (admins can open
the desktop, power it on and off, and see everything on it, for 24 hours,
with a notification each time). Confirming calls
`POST /api/vm/computers/:id/admin-access {"allow": true}`; "Turn off" sends
`{"allow": false}`, which also closes any open admin desktop on that computer.
Only the computer's own owner can call it — not an admin, not the site owner.

Grants live in the `vm_admin_access_grants` table (`mitch_lib::vm_access`):
one row per computer with the owner it was granted by, granted and expiry
times. A grant expires after 24 hours (`GRANT_DURATION_MS`), and only
counts while the computer still belongs to the owner who made it, so
reassigning a computer drops it.

When an admin opens the desktop under a grant, the owner gets a
"Admin Used Your Computer" notification, and `ADMIN_VM_USED` is written to the
VM audit log. Allowing and withdrawing are logged as `ADMIN_ACCESS_ALLOWED` /
`ADMIN_ACCESS_REVOKED`.

## What admins can still do without a grant

Fleet management on `/admin/vms/` that doesn't open the computer: assign,
unassign, edit resources, restrict VM/BYO-OS use, keep-alive, delete. The
fleet list shows "Owner only" instead of Open/power buttons for computers they
can't open.

## Tests

`vm_security::tests::access_ladder`, `vm_access::tests`, and the
`vm_record_allowed_for_actor` cases in `routes/vm.rs` tests (no grant, grant,
reassigned VM, non-admin with a grant, owner-locked VM with a grant).
