# Graphical computer portal

The customer entry point is `/vms/`; `/vms/desktop/?id=<application-record-id>` opens the real VM display. Owners, co-owners, and administrators manage computers at `/admin/vms/`. Moderators do not have infrastructure administration access.

## Architecture and data

The portal extends the existing Bun server, static HTML/CSS/JavaScript frontend, and website sessions. It uses the bundled noVNC client. There is no additional authentication service, ORM, frontend framework, or separately exposed console server.

`lib/proxmox_desktop.js` owns REST calls, guest-agent status, console sessions, and provisioning. `lib/vm_security.js` contains ownership/session policy. `lib/data_store.js` creates additive `virtual_machines` and `vm_audit_logs` tables in the existing `data/mitchpro.db` SQLite database. Existing account and customization data remain in the same datastore. Approved legacy VM assignments migrate on startup; repeated starts do not create duplicate VM rows.

VM operations resolve the application record first, then authorize the signed-in owner or administrator. Proxmox node, numeric VM ID, host, port, and upstream WebSocket URL come only from trusted server state. The normal-user API omits hypervisor connection details.

An authenticated console request obtains a short-lived Proxmox VNC ticket and creates a single-use, 75-second application connection handle. The browser connects to the website's `/api/vm/desktop/ws` endpoint; Bun connects to the Proxmox `vncwebsocket` endpoint using its server-side API token. The temporary VNC password needed by the RFB protocol may be delivered to the authenticated noVNC client; it is not an API token, not a persistent login credential, and must never be cached or logged. Application restart discards pending connection handles; reconnect creates a fresh one.

Back up the SQLite database using SQLite's backup mechanism or a stopped application, together with the existing session key and data files. VM disks require separate Proxmox backups. Unassigning a computer removes customer access; it does not erase its guest disk or delete the VM.

## Server configuration

Use deployment secrets or the private server `.env`, never files below `webserver/`:

```dotenv
PROXMOX_HOST=192.168.100.1
PROXMOX_PORT=8006
PROXMOX_NODE=tartarus
PROXMOX_TOKEN_ID=portal@pve!customer-portal
PROXMOX_TOKEN_SECRET=<secret supplied outside source control>
PROXMOX_VERIFY_TLS=true
PROXMOX_TLS_SERVERNAME=mitch.pro
PROXMOX_DESKTOP_TEMPLATES=9010
HIDE_VM_FEATURES=0
```

`PROXMOX_TLS_SERVERNAME` validates the host's certificate for `mitch.pro` while the connection uses the private address. Install a trusted CA on the application host if replacing the existing publicly trusted certificate. Do not disable certificate validation to hide a hostname mismatch. The legacy server-side `PVE_*` settings remain supported for older VM features; a complete new token configuration takes precedence for the graphical portal.

Grant a dedicated, privilege-separated Proxmox API token only the permissions needed for the customer pool, permitted templates, target storage/network, and node status. Both the token and its parent user must have the required ACLs. Do not give it root's unrestricted role. Check effective permissions with `pveum user token permissions` after configuring ACLs. The portal needs guest audit/console/power/configuration/clone permissions, pool membership, storage allocation for cloning/resizing, permitted network attachment, and node audit for capacity. Existing unrelated VMs must remain outside that scope.

Keep port 8006 private. Customers only need the website's HTTPS/WSS endpoint. The existing outer nginx proxy must forward `Upgrade` and `Connection` for WebSockets; Caddy's `reverse_proxy` handles the upgrade automatically. Do not add VNC port forwarding, expose guest port 5900, or iframe the Proxmox administrative UI.

## Linux desktop templates

### Ubuntu fallback used for automation

The previously named `linux-template-vnc` (9000) was inspected and booted to a Debian terminal, so its name alone is not evidence of a desktop. It must not be offered as a ready graphical Linux Mint computer.

Template 9010 is the Ubuntu 24.04 LTS desktop fallback. Build it from a verified [Ubuntu cloud image](https://cloud-images.ubuntu.com/noble/current/), then install the desktop and guest-agent packages before converting it to a template:

```sh
sudo apt-get update
sudo apt-get install -y ubuntu-desktop-minimal qemu-guest-agent
sudo systemctl enable qemu-guest-agent
sudo systemctl set-default graphical.target
```

Verify a browser, file manager, settings, application menu, terminal, graphical login, and normal keyboard/mouse input before sealing the template. Ubuntu's desktop package supplies the desktop session; a server cloud image by itself does not. Keep a real virtual VGA display (`std` or a tested QXL configuration); a serial-only display will produce a terminal in noVNC.

Use a VirtIO network adapter, the selected customer bridge, a cloud-init drive, and a SCSI system disk. Enable the QEMU Guest Agent in both the guest and VM options. Clean cloud-init instance state and machine identity immediately before shutdown so clones receive distinct identities and their own first-boot configuration. Do not preserve a template operator's password, SSH keys, browser profile, or history in customer clones. Convert the powered-off, validated guest to a template and add its ID to `PROXMOX_DESKTOP_TEMPLATES`.

The admin workflow clones this template, applies CPU/RAM/disk settings, prepares supported cloud-init user/network settings, starts the guest, and stores website ownership. Use 4 vCPU, 4 GB RAM, and 40 GB disk initially. Guest-agent IP discovery may remain blank while the guest boots. A Proxmox disk expansion also requires a guest filesystem capable of growing into the new space; verify `cloud-init`/`growpart` behavior when changing the base image.

### Preferred Linux Mint Cinnamon workflow

Linux Mint's supported starting point is its verified installation ISO. Follow the [official installation guide](https://linuxmint-installation-guide.readthedocs.io/en/latest/) and its [OEM installation workflow](https://linuxmint-installation-guide.readthedocs.io/en/latest/oem.html) for a reusable installation that lets each recipient create their own desktop account.

1. Create a new, unused QEMU VM with 4 vCPU, 4 GB RAM, a 40 GB SCSI disk, VirtIO network, and a graphical VGA display. Attach the verified Cinnamon ISO and perform an OEM installation through the Proxmox console.
2. Install updates, `qemu-guest-agent`, and the desired browser/utilities. Enable the agent and confirm a normal graphical login and Cinnamon session through the website console.
3. Prepare the OEM installation for the next user using Mint's supported preparation step. Remove the installation ISO, power off, and convert the VM to a template.
4. Make an isolated clone and complete the recipient's first-run account setup. Confirm unique hostname/machine identity, networking, display, input, and guest-agent status before advertising the template to customers.
5. Register the validated template ID in the server allowlist. Use the manual first-run OEM setup for Mint unless cloud-init user/network provisioning has separately been installed and tested in the guest. Adding a cloud-init drive alone does not make a Mint ISO installation cloud-init aware.

Do not add an untested distribution conversion script to the automatic provisioning path. Ubuntu remains the automated fallback until a Mint template passes this workflow.

## Deployment and verification

The existing `master` push workflow runs unit tests and triggers `/usr/local/bin/deploy.sh` on the application server. That script builds the inactive Docker slot, checks its HTTP startup, switches Caddy, and stops the old slot. Both blue and green containers must receive the portal's environment settings. A deployment or application restart disconnects an active desktop transport; the VM continues running and the frontend can reconnect.

Before deploying:

```sh
bun install --frozen-lockfile
bun run test:unit
node --check server.js
node --check lib/proxmox_desktop.js
node --check lib/vm_security.js
```

This repository is plain JavaScript with static frontend files. It does not currently define TypeScript, lint, or frontend production-build scripts. The production build is the Docker image build; validate it through the existing deployment or `docker compose build` on a machine with Docker. Do not claim nonexistent checks passed.

Run integration tests only in a local/test checkout. Its harness backs up and restores that checkout's data directory and uses `NODE_ENV=test`; it must not point at production data or a production `BASE_URL`. Unit tests mock upstream failures, stopped guests, authorization policy, and temporary sessions. HTTP tests require exact 403 responses for User A attempting to view, start, stop, restart, shut down, or open a console for User B, and deny normal-user access to every VM admin route.

After deployment, use an assigned test desktop to verify the customer and administrator pages, graphical login, keyboard typing, mouse clicks, scaling/fullscreen, reconnect after application restart, power transitions, guest IP reporting, and ownership isolation. Check the browser's served JavaScript and console-session response for persistent secrets; no API token, Proxmox cookie, or upstream destination may appear. An opaque application connection handle and temporary RFB credential are expected only in an authenticated, non-cacheable console response.

## Operational notes

- An offline computer must be started before opening its desktop. A browser reconnect does not start or reboot a guest.
- Normal shutdown requests guest cooperation. Force Stop is disruptive and can lose unsaved data; its interface requires confirmation.
- Slow boot can temporarily show an unknown IP or connection failure. Confirm guest-agent service status and graphical display configuration before changing the web proxy.
- `VM_CREATED`, `VM_ASSIGNED`, `VM_UNASSIGNED`, power actions, and `DESKTOP_OPENED` are recorded with actor, VM, timestamp, and success/failure. Never add credentials or full upstream URLs to audit details.
- Treat reassignment as handing an existing computer and its files to a different person. Create a fresh clone for a new customer when prior guest data should not be shared.
- Keep template updates separate from active customer machines. Validate a fresh clone before changing the template allowlist.

## Desktop repair and automatic sign-in

The production desktop allowlist is `PROXMOX_DESKTOP_TEMPLATES=9011,9010`, with `PVE_TEMPLATE_LINUX=9011`. Template 9011 is the cleaned Ubuntu 24.04 desktop image with video decoders and a boot service that reads the recipient's cloud-init username before GDM starts. Template 9010 remains available for existing records. Both are in the `sandboxes` pool so the existing pool ACLs apply.

New user computers get a random guest password. Website authentication opens the console, the guest signs in automatically, and sudo works without a second password. Custom OS installers retain their own account setup. Repair desktop applies the same configuration to existing managed Linux computers, preserves active desktop sessions, and checks that a graphical session actually starts. It also installs missing video decoders. A failed repair remains visible and can be retried; it does not block the console. Opening or reconnecting the console does not restart or reconfigure the guest display manager.

Authenticated administrators can open and manage all registered computers, including those belonging to other administrators. Owner approval and owner notifications have been removed. Administrative operations remain recorded in the internal VM audit log. Ordinary accounts can only access their assigned computers. Desktop tickets remain single-use and bound to the account, session, VM, node, and ownership; account bans close connected consoles.

To build another image, use a dedicated VM in the 9000–9999 range, apply `rust/crates/mitch-server/guest/desktop-setup.sh` with a temporary build username, verify the desktop and video playback, then run `tools/seal-managed-desktop-template.sh <vmid> <build-user>` on the Proxmox host. The sealer removes the temporary user, credentials and SSH host keys, cleans cloud-init/machine identity, and converts the stopped VM into a template. Validate a fresh clone with a different username before changing the allowlist. Do not edit a template disk used by linked clones.

Validation for 9011: a fresh recipient clone reached graphical.target in about 20 seconds, logged in with its own username, retained no build-user home, and had no failed system services. The prior repaired clone reached graphical.target in about 17 seconds on a subsequent cold boot. These timings cover the system target; the first desktop session takes additional time to load.

## Resource edits and Windows installers

Owner and admin resource edits use the normalized Proxmox runtime `state` field.
Only a fresh `stopped` state permits a change; running VMs return 409, and an
unavailable or unknown state returns 503. The saved assignment status is not a
power-state source. A shared VM operation lock prevents competing power and
resource edits in the app. Values must be finite whole numbers, and disks cannot
shrink. Proxmox is updated before the account record is saved; host errors do not
write speculative specs. A database error after a host success tells the admin
to retry the same values. Proxmox itself has no transaction spanning CPU, memory,
and disk changes, so a multi-resource failure can require such a retry.

Resource limits: Only the primary site owner can enter unlimited custom hardware
resources. For all other users—including co-owners and administrators—custom VM
resources and admin resource edits are strictly capped at 8 CPU cores, 16 GB RAM
(16384 MB), and 128 GB disk storage.

`/admin/vms/` provides a resource dialog instead of three prompts. The old
`downgrade-specs` URL remains a deployment compatibility alias for `resources`.
Fleet management actions are grouped in a menu, with operational capacity and
recent audit records kept visible. Personal usage charts are no longer collected
or exposed in this page.

New Windows presets use Q35, OVMF UEFI, a 4M EFI variables disk with pre-enrolled
keys, TPM 2.0, host CPU, at least 2 cores / 4 GB RAM / 64 GB disk, standard display,
and SATA/E1000e devices with inbox Windows drivers. Linux/custom defaults remain
separate. See [Proxmox's Windows 11 recommendations](https://pve.proxmox.com/wiki/Windows_11_guest_best_practices).

`ensure_optimized_vm_config` forces a display adapter on every VM at
power-on: `virtio,memory=128` by default, `virtio-gl` (VirGL — GPU-
accelerated 3D, not just the plain virtio-gpu 2D framebuffer) for ChromeOS
clones and owner-owned VMs, and nothing at all for Windows guests
(`ostype` starting with `win` — vanilla Windows has neither driver).
ChromeOS-clone detection (VM record's `templateVmid` matching the
configured `chromeos_template_vmid`) and owner detection (`is_owner_email`
on the record's `ownerEmail`) both have to happen in `power()`, not inside
`ensure_optimized_vm_config` itself: the bare Proxmox config that function
reads has no owner or "cloned from template X" information, and
`ProxmoxDesktopService` has no `DataStore` access to look either up
itself. Without the Windows exception, a Windows guest's display would be
silently overwritten (black screen) on its next restart;
`clone_chromeos_desktop` also sets `virtio-gl` directly at clone time, so
a fresh ChromeOS clone doesn't wait for its first manual restart to get it.

## ChromeOS (BYO-OS preset)

ChromeOS Flex's distributed image is a raw disk image (what you'd `dd`
onto a USB stick), not a bootable-CD ISO, so it can't go through the
normal BYO-OS flow (attach an ISO as `media=cdrom`, boot the installer) —
a from-scratch attempt using Proxmox's `import-from` disk option failed
Proxmox's own parameter validation. Instead, an admin builds and installs
ChromeOS by hand in a regular VM, converts it to a Proxmox template
(`qm template <vmid>`, after shutting it down), and sets
`PROXMOX_CHROMEOS_TEMPLATE_VMID` to that vmid. The BYO-OS preset
("chromeos") then clones that template directly
(`clone_chromeos_desktop`) — the same proven clone mechanism the normal
desktop template uses, just without any cloud-init/credential setup
(ChromeOS has no local Linux user to seed; each clone boots to the
template's own Google sign-in, same as the original).

Until `PROXMOX_CHROMEOS_TEMPLATE_VMID` is set, the "chromeos" preset is
simply absent from `GET /api/vm/byo-os/presets` and `create-preset`
returns 503 for it — there's no broken-looking option sitting in the UI.

Existing custom VMs offer **Windows installer compatibility**. This is an
explicit, authenticated, CSRF-protected action on the user's own stopped VM. It
preserves disks and existing TPM/EFI state, grows storage to 64 GB when needed,
and raises CPU/RAM minimums before returning success. Firmware changes can make
an existing legacy BIOS installation require reinstalling; the confirmation
explains that risk. Deployment does not silently convert existing guests. Guest
CPU compatibility still depends on the physical host. Detaching an ISO selects
the actual SCSI/SATA/VirtIO boot disk instead of assuming SCSI.

### ChromeOS keep-alive (admin-only)

`POST /api/admin/vms/keep-alive` sets a per-VM `keep_alive` column
(`virtual_machines` table — a real column, not a JSON flag) that exempts one
computer from `workers_vm.rs`'s inactivity *and* daily/session-uptime
watchdog entirely, so it can run 24/7. Full admins only; the target must
be an actual ChromeOS clone (`templateVmid` matching
`PROXMOX_CHROMEOS_TEMPLATE_VMID`) or the request is rejected — it's not a
general-purpose "never shut down" switch for any VM. Toggled from the
admin fleet page's per-row "Manage" menu, which only shows the button for
ChromeOS rows. `set_vm_keep_alive` is a dedicated narrow `UPDATE`,
deliberately not folded into `upsert_virtual_machine` (which overwrites
every other column on every ordinary create/recreate/reassign and would
silently reset this back to `false`).

### Owner 24-Machine ChromeOS Fleet & Controller

To support automated batch testing and owner workflows, an owner-exclusive 24-machine ChromeOS fleet can be provisioned and controlled via `/admin/vms`:
- **Hardware Specs**: 2 CPU cores, 4 GB RAM (4096 MB), 32 GB disk, `keep_alive = 1` (exempt from inactivity shutdown), owner-locked (`is_vm_owner_locked`).
- **VMID Allocation**: VMIDs start strictly at 300 (avoiding any collision with VMIDs below 300), checking both Proxmox's live cluster guest registry and SQLite database to allocate contiguous available IDs.
- **Provisioning Endpoint**: `POST /api/admin/vms/fleet/create-chromeos` provisions up to 24 machines named `chromeos-fleet-01` through `24`, cloned from `PROXMOX_CHROMEOS_TEMPLATE_VMID`. Strictly owner-only.
- **Batch Power Endpoint**: `POST /api/admin/vms/fleet/power` accepts `action: "start" | "shutdown" | "restart" | "force-stop"`, executing power transitions across all 24 machines concurrently.
- **Fleet List Endpoint**: `GET /api/admin/vms/fleet/chromeos` returns all 24 fleet records with real-time status from Proxmox.
- **Safe Fleet Delete Endpoint**: `POST /api/admin/vms/fleet/delete-chromeos` deletes owner fleet machines. Strictly requires that any target machine has `"chromeos"` in its name (friendlyName or hostname), is owned by the owner, is NOT the template VMID, and is NOT in the protected provisioning pool `200..=210`. Supports `{ id }` for individual deletion, `{ all: true }` for batch fleet deletion, and `{ localOnly: true }` to remove records from mitch.pro database only without contacting Proxmox.
- **Multi-Screen Controller (`/admin/vms/fleet/`)**:
  - Gated to `is_owner_email` at both the HTML route handler and API layer.
  - Responsive grid rendering live noVNC RFB streams of all 24 machines simultaneously.
  - Interactive single-machine zoom mode to inspect or interact with any individual machine, with an instant toggle back to the 24-screen grid.
  - Integrated **Mouse Jiggler**: automatically injects micro pointer move events every 10 seconds across all connected sessions to prevent ChromeOS idle lock, sleep, or screensaver.
  - Global "Turn On All" and "Power Off All" controls.

### Owner-only test-persona account

`newaccount123@student.rjuhsd.us` (`mitch_lib::auth::OWNER_TEST_ACCOUNT_EMAIL`)
is a real account the owner uses to test the student experience, not a
real student. `is_vm_owner_locked` (the single shared check behind every
"can an admin touch this VM" gate — desktop access, reassign, unassign,
delete, resource edits) treats its VM exactly like a primary owner's own
VM: only a real owner, or that account's own session, can touch it; no
co-owner or other admin can, unlike every other VM on the site. It's also
excluded from the leaderboard (`ranked_players`), the same way tester
accounts are.

## Start capacity cap and inactivity deletion

**Capacity cap on starts.** Starting or restarting a stopped computer through `/api/vm/computers/<id>/power` is checked against the fleet cap (`VM_FLEET_MAX_CORES` / `VM_FLEET_MAX_MEMORY_MB`, via `can_accommodate_vm_resources`) and returns 409 `capacity_limit_reached` when it would exceed it. The cap applies to admins too, so one admin cannot power on every customer VM. Admin-owned computers are not counted in the cap and skip the check. Starts Proxmox has accepted but that are not yet `running` are reserved in memory (`reserve_pending_start`, 2-minute TTL, dropped once the guest shows as running), so a burst of requests cannot all read the same running total.

**30-day inactivity deletion.** `virtual_machines.last_active_at` (ms) is set when the *owner* opens the desktop or starts the computer; admin actions on someone else's computer do not reset it. `delete_inactive_vms_worker` (`workers_vm.rs`, hourly, first run an hour after boot) deletes assigned computers whose `MAX(last_active_at, created_at)` is older than 30 days. It skips keep-alive computers, computers owned by owners/co-owners/admins, owner-locked accounts, any computer not confirmed `stopped` by Proxmox, and any with an open desktop socket. It destroys the guest first and keeps the record if Proxmox refuses. Each outcome is audited as `VM_DELETED_INACTIVE`. The migration that adds the column starts every existing computer's clock at deploy time.

The owner also has a private container (VMID 500) that is hidden from every other account, see [owner-container.md](owner-container.md).
