# Proxmox API privileges

Every Proxmox API call the portal makes, for building a least-privilege role. The portal uses one API token (`PROXMOX_TOKEN_ID` / `PROXMOX_TOKEN_SECRET`, `mitch-portal@pve!website`). Both the token and its parent user need the ACLs below (use privilege separation and check with `pveum user token permissions`).

Code: `rust/crates/mitch-server/src/proxmox_desktop.rs` (`ProxmoxDesktopService`) and the raw calls in `rust/crates/mitch-server/src/routes/vm.rs`. Paths are relative to `/api2/json`; `{node}` is `PROXMOX_NODE` (`tartarus`), `{type}` is `qemu` or `lxc`.

The privileges below come from the Proxmox API documentation, not from a run against a locked-down token. Rows marked **verify** are the ones most likely to differ by Proxmox version; confirm them on the host (a missing one shows up as a logged 403, see the end).

## Expected ACL scope

| ACL path | Privileges |
| --- | --- |
| `/pool/sandboxes` (propagate) | `VM.Allocate`, `VM.Audit`, `VM.Clone`, `VM.Console`, `VM.PowerMgmt`, `VM.Monitor` (PVE 8) or `VM.GuestAgent.Audit` + `VM.GuestAgent.Unrestricted` (PVE 9), `VM.Config.CPU`, `VM.Config.Memory`, `VM.Config.Disk`, `VM.Config.CDROM`, `VM.Config.Cloudinit`, `VM.Config.Network`, `VM.Config.HWType`, `VM.Config.Options`, `Datastore.AllocateSpace` |
| `/sdn/zones/students/stunet` | `SDN.Use` |
| `/nodes/tartarus` | `Sys.Audit` |
| `/storage/local` | `Datastore.AllocateTemplate` (ISO upload and delete), `Datastore.Audit` (attach an ISO) |
| `/storage/local-lvm` | `Datastore.AllocateSpace` (VM and container disks, EFI/TPM state) |
| `/vms/{template}` for each `PROXMOX_DESKTOP_TEMPLATES` id and `PROXMOX_CHROMEOS_TEMPLATE_VMID` | `VM.Clone`, `VM.Audit` (not needed separately if the templates are in the `sandboxes` pool) |

## Endpoints

### Read

| Method | Endpoint | Used for | Needs |
| --- | --- | --- | --- |
| GET | `/cluster/resources?type=vm` | guest list | none at the path; rows are filtered to guests the token has `VM.Audit` on |
| GET | `/nodes/{node}/status` | capacity | `Sys.Audit` on `/nodes/tartarus` |
| GET | `/nodes/{node}/{qemu\|lxc}` | free VM id allocation | `VM.Audit` (filtered) |
| GET | `/nodes/{node}/{type}/{vmid}/config` | config reads | `VM.Audit` on `/vms/{vmid}` |
| GET | `/nodes/{node}/{type}/{vmid}/status/current` | power state | `VM.Audit` |
| GET | `/nodes/{node}/tasks/{upid}/status` | wait for clone/create | none for the token's own tasks |
| GET | `/nodes/{node}/qemu/{vmid}/agent/network-get-interfaces` | guest IP | `VM.Monitor` (PVE 8) / `VM.GuestAgent.Audit` (PVE 9) |
| GET | `/nodes/{node}/qemu/{vmid}/agent/get-host-name` | hostname | same |
| POST | `/nodes/{node}/qemu/{vmid}/agent/ping` | agent readiness | same |
| GET | `/nodes/{node}/qemu/{vmid}/agent/exec-status` | result of an exec | same |

### Guest lifecycle

| Method | Endpoint | Used for | Needs |
| --- | --- | --- | --- |
| POST | `/nodes/{node}/{type}/{vmid}/status/{start,stop,shutdown,reboot}` | power | `VM.PowerMgmt` |
| POST | `/nodes/{node}/{type}/{vmid}/vncproxy`, then WebSocket `.../vncwebsocket` | consoles (no VM IP needed) | `VM.Console` |
| POST | `/nodes/{node}/qemu/{vmid}/agent/exec` | first-boot setup inside the guest | `VM.Monitor` (PVE 8) / `VM.GuestAgent.Unrestricted` (PVE 9) |
| POST | `/nodes/{node}/qemu/{template}/clone` (`pool=sandboxes`, `full=0`) | new computer from a template | `VM.Clone` on the template, `VM.Allocate` on `/pool/sandboxes`, `Datastore.AllocateSpace` on the target storage (**verify** for linked clones) |
| PUT/POST | `/nodes/{node}/qemu/{vmid}/config` | cores, memory, balloon, `cpu`, `vga`, `rng0`, `agent`, `name`, cloud-init (`ciuser`, `cipassword`, `ipconfig0`, delete `sshkeys`), `net0` rewrite, ISO eject, TPM/EFI | `VM.Config.CPU`, `.Memory`, `.HWType` (vga/bios/machine), `.Options`, `.Cloudinit`, `.CDROM`, `.Network` (+ `SDN.Use` on the bridge for `net0`), `.Disk`; `Datastore.AllocateSpace` for `efidisk0`/`tpmstate0` |
| PUT | `/nodes/{node}/qemu/{vmid}/resize` | grow a disk | `VM.Config.Disk`, `Datastore.AllocateSpace` |
| POST | `/nodes/{node}/qemu` | create a BYO-OS VM (`pool=sandboxes`, `net0`, ISO on `ide2`, blank disk) | `VM.Allocate` on `/pool/sandboxes`, the `VM.Config.*` set above, `SDN.Use` on `stunet`, `Datastore.AllocateSpace` on `local-lvm`, `Datastore.Audit` on `local` (ISO) |
| POST | `/nodes/{node}/lxc` (`pool=sandboxes`, `net0=...,bridge=stunet,ip=dhcp`) | student container | `VM.Allocate` on `/pool/sandboxes`, `SDN.Use` on `stunet`, `Datastore.AllocateSpace` on `local-lvm`, `Datastore.Audit` on the template storage |
| DELETE | `/nodes/{node}/{type}/{vmid}?purge=1&destroy-unreferenced-disks=1` | delete a computer and its disks | `VM.Allocate` on `/vms/{vmid}`. **Removes volumes:** **verify** whether the token also needs `Datastore.Allocate` on `/storage/local-lvm`. |

### Storage

| Method | Endpoint | Used for | Needs |
| --- | --- | --- | --- |
| POST | `/nodes/{node}/storage/{storage}/upload` | ISO upload (`PROXMOX_ISO_STORAGE`, `local`) | `Datastore.AllocateTemplate` on `/storage/local` |
| DELETE | `/nodes/{node}/storage/{storage}/content/{storage}:iso/{file}` | remove an uploaded ISO | `Datastore.AllocateTemplate` on `/storage/local` (**verify**: some versions ask for `Datastore.Allocate`) |

## Things that go beyond the expected scope

- **Datastore.Allocate (deleting volumes):** guest deletion with `destroy-unreferenced-disks=1` and ISO deletion. If the token only has `Datastore.AllocateSpace`, deleting a computer may fail with a 403 and leave the disks behind. Decide deliberately whether to grant `Datastore.Allocate` on `/storage/local-lvm` rather than adding it to silence an error.
- **Pool.Allocate:** not used. The portal never creates or edits pools; it only passes `pool=sandboxes`.
- **Sys.Modify:** not used.
- **Outside `/pool/sandboxes`:** the template VMs it clones (`VM.Clone`, plus `VM.Audit` to read their config; the template build script puts them in the pool, which removes the need for a separate ACL) and the `/storage/*` and `/nodes/tartarus` paths above.
- **Not an API call:** `attach_sshd_hook_to_lxc` (`routes/vm.rs`) runs `mitch-attach-hook <vmid>` over SSH to the Proxmox host using `PVE_SSH_*`. It has no API privilege equivalent and is not covered by the token.

## When a privilege is missing

Proxmox answers 403 with a message such as `Permission check failed (/vms/200, VM.PowerMgmt)`. The portal logs `[proxmox] 403 on <METHOD> <endpoint>: <message>` at error level and returns `PERMISSION_DENIED` ("the portal's Proxmox token lacks a required permission", HTTP 503) to the UI. It does not retry and does not fall back to another token. Add the privilege named in the log to the role, or leave it out on purpose and accept the failure.
