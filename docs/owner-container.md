# Owner container (VMID 500)

A Proxmox container that only the owner can see or use. It is created by hand
in Proxmox (the site never creates or deletes it) and is opened with the normal
VM desktop page in owner mode, `/vms/desktop/?container=owner` (fullscreen,
clipboard, Ctrl+Alt+Del, power menu, cursor options, reconnect all work as on
any computer). Cards on the admin page's Site operations tab and the staff page
link there for the owner only. The page asks for the admin passphrase in a
dialog when it is needed.

## Who can see it

Only an owner (`auth::is_owner_email`). Admins and developers must not learn it
exists, so:

- `/api/admin/owner-container*` returns the same 404 as an unknown route to
  everyone except the owner (`routes/admin/owner_container.rs`); the shared
  desktop page in owner mode then shows "could not be found". Admin-passphrase gating still applies to the API first.
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
| `POST /api/admin/owner-container/console` | `{mode}` (`desktop` default, or `text`) | One-time noVNC session (`socketPath`, `credentials`); container must be running |

POSTs require a same-origin request. `GET` also returns `desktopConfigured`.

## Consoles

- **Text console** (`mode: "text"`): Proxmox's `vncproxy` for the guest's own
  type through the normal `/api/vm/desktop/ws` bridge. For an LXC this is only
  the text login; Proxmox cannot show an LXC's GNOME session.
- **Desktop** (`mode: "desktop"`, used when `OWNER_CONTAINER_VNC_HOST` is set):
  the server opens a TCP connection to the VNC server running inside the
  container and relays it to the browser's noVNC (`bridge_vnc_tcp`). The
  container must be reachable from the webserver container/host.

Server environment (`.env`; passed through in `docker-compose.yml`):

| Variable | Meaning |
| --- | --- |
| `OWNER_CONTAINER_VNC_HOST` | IP or hostname of the container's VNC server. Empty disables the desktop button. |
| `OWNER_CONTAINER_VNC_PORT` | VNC port, default `5901` (display `:1`). |
| `OWNER_CONTAINER_VNC_PASSWORD` | VNC password; sent to the owner's browser over TLS when noVNC asks. Classic VNC auth uses only the first 8 characters. |

The host comes only from the environment, never from a request.
Sessions (both modes) carry `ownerContainer: true`, which makes the bridge (and its periodic
re-authorization in `vm_desktop_socket_authorized`) accept them for an owner
session only, with no database record.

## Untested against real Proxmox

The code compiles and the unit tests pass, but it was written without a Proxmox
host to test against. Check status, power and the console once the container
exists.
