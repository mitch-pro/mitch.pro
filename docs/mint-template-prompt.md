# Prompt: build the Linux Mint default desktop template

Hand this to the AI agent running on the Proxmox host. The result replaces the
Ubuntu 24.04 template (9011) as the default for new managed computers. Background
is in [computer-portal.md](computer-portal.md).

```text
Build a new default desktop template for mitch.pro based on Linux Mint 22.x
Cinnamon (Ubuntu 24.04 base), replacing the Ubuntu 24.04 template (9011). New
managed computers are cloned from it. Work carefully: this runs on a production
Proxmox host.

Read first
- /home/user/mitch.pro/docs/computer-portal.md, especially "Desktop repair and
  automatic sign-in" and "Preferred Linux Mint Cinnamon workflow".
- rust/crates/mitch-server/guest/desktop-setup.sh (the guest configuration the
  server applies and re-applies on "Repair desktop").
- tools/seal-managed-desktop-template.sh (seals and templates a build VM).
- tools/prepare-ubuntu-desktop-template.sh and tools/seal-ubuntu-desktop-template.sh
  for how 9010/9011 were built.

Constraints
- Use a dedicated build VM with a free ID in 9000-9999. Do not touch 9010, 9011,
  or any customer VM. Do not modify a template disk that linked clones use.
- Use a freshly downloaded, checksum- and signature-verified Linux Mint Cinnamon
  ISO, or the Ubuntu 24.04 cloud image with Mint's packages layered on. Pick
  whichever gives a working cloud-init username flow with less hackery; the
  template must behave like 9011 from the server's point of view.
- 4 vCPU, 4 GB RAM, 40 GB SCSI disk, VirtIO net on the customer bridge, a
  cloud-init drive, real VGA (std), QEMU guest agent enabled in guest and VM
  options. A serial-only display shows a terminal in noVNC and is wrong.
- Template must be in the `sandboxes` pool so existing ACLs apply.

Requirements (what the server relies on)
1. Cloud-init is installed and working. The server clones the template, sets a
   per-user username and random password via cloud-init, and expects the guest
   to sign in automatically as that user. 9011 does this with a boot service
   that reads the cloud-init username before the display manager starts. Mint
   uses LightDM, not GDM, so port that service and configure LightDM autologin
   for the recipient user. Sudo must work without a second password.
2. Cinnamon session starts reliably, with browser, file manager, terminal,
   settings and app menu working. Include video decoders / codecs (Mint ships
   most of them; verify playback in the browser).
3. Apply rust/crates/mitch-server/guest/desktop-setup.sh with a temporary build
   username. If it is Ubuntu/GDM-specific, adapt it to also handle Mint/LightDM
   without breaking Ubuntu guests (existing computers still use it), and tell me
   what you changed so I can commit it.
4. Seal with tools/seal-managed-desktop-template.sh <vmid> <build-user>: removes
   the build user, credentials, SSH host keys, browser history, cloud-init
   instance state and machine-id, then converts the stopped VM to a template.
   No build-user password, SSH key, browser profile or history in the template.
5. Remove the Ubuntu branding leftovers that matter to users (the "Ubuntu" MOTD
   ads, Ubuntu Pro/ESM nags, snap-store promos) and make sure unattended
   upgrades don't reboot the guest mid-session.

Validate before telling me it's done
- Clone the new template with a *different* username than the build user,
  using the same cloud-init path the server uses. Confirm: boots to
  graphical.target in a similar time to 9011 (~20s), auto-logs-in as the new
  user, Cinnamon is on screen via the website noVNC console, keyboard and mouse
  work, unique hostname and machine-id, network up, guest agent reports an IP,
  no failed systemd units, no build-user home left behind.
- Cold-reboot the clone once and confirm it still works.
- Delete the test clone afterward.

Deliver
- The new template ID, and the exact env changes: add it to
  PROXMOX_DESKTOP_TEMPLATES (keep 9011 and 9010 listed for existing records) and
  set PVE_TEMPLATE_LINUX to the new ID. Do NOT change either variable yourself;
  give me the values and I'll apply them.
- Any script changes as a diff, plus the validation results, so I can update
  docs/computer-portal.md.
```
