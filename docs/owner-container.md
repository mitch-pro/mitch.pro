# Owner container (VMID 500)

A Proxmox container that only the owner can see or use. It is created by hand
in Proxmox (the site never creates or deletes it) and is reached at
`/admin/vms/owner-container/` (a card on the admin page's Site operations tab
links there for the owner).

## Who can see it

Only an owner (`auth::is_owner_email`). Admins and developers must not learn it
exists, so:

- `/api/admin/owner-container*` and the page return the same 404 as an unknown
  route to everyone except the owner (`routes/admin/owner_container.rs`,
  `handler.rs`). Admin-passphrase gating still applies to the API first.
- `ProxmoxDesktopService::list_guests` drops VMID 500, so none of the admin
  fleet/VM lists, capacity numbers or workers include it. Only
  `list_guests_all` (used by this module) returns it.
- `next_available_vmid` reserves 500 so a new computer is never assigned it.
- There is **no** `virtual_machines` row for it, so the inactivity/uptime
  workers and every admin VM action can't reach it. The id is in the allocator
  range (200-999) but is not managed by it.
- This module does not write to `vm_audit_logs` or `tracing` (admins and
  developers can read both).
- Everyone with repo access can still read the source; this hides it from the
  site's UI and APIs, not from the code.

## API (owner only)

| Endpoint | Body | Effect |
| --- | --- | --- |
| `GET /api/admin/owner-container` | | `{exists, name, type, status, cpuCores, memoryMb, diskGb, uptime}` from Proxmox |
| `POST /api/admin/owner-container/power` | `{action}` (`start`, `shutdown`, `restart`, `force-stop`) | Proxmox power action |
| `POST /api/admin/owner-container/console` | | One-time noVNC session (`socketPath`, `credentials`); container must be running |

POSTs require a same-origin request. The console uses Proxmox's `vncproxy` for
the guest's own type (`lxc`) through the normal `/api/vm/desktop/ws` bridge.
Sessions carry `ownerContainer: true`, which makes the bridge (and its periodic
re-authorization in `vm_desktop_socket_authorized`) accept them for an owner
session only, with no database record.

## Untested against real Proxmox

The code compiles and the unit tests pass, but it was written without a Proxmox
host to test against. Check status, power and the console once the container
exists.
