# Suggested System Improvements & Roadmap

This document outlines high-impact improvements and enhancements across the platform based on current architecture, recent operational fixes, and administrator workflows.

---

## 1. 🖥️ VM Fleet Management & Admin UX (`/admin/vms`)

### 1.1 Responsive Status Transitions (Progressive Polling / WebSockets)
* **Current Behavior**: When an administrator triggers a power action (Start, Restart, Shutdown, Force Stop), the UI waits a static 6 seconds before calling `load()`.
* **Proposed Enhancement**:
  * Implement short progressive polling: query `/api/admin/vms/overview` every 1.5 seconds for up to 15 seconds after a state change until the status updates to `running` or `stopped`.
  * Alternatively, push VM power status updates over the existing WebSocket notification channel.
  * **Benefit**: Eliminates sluggish UI lag and gives instant feedback on computer lifecycle transitions.

### 1.2 Fleet Filters & Bulk Administrative Actions
* **Proposed Features**:
  * **Quick Filter Badges**: Filter the VM table by status: `All`, `Running`, `Stopped`, `Provisioning`, `Failed`.
  * **Search Bar**: Live search by VM ID, hostname, or owner email.
  * **Bulk Actions**:
    * "Shut Down All Inactive": Gracefully powers off student VMs with no active desktop sessions.
    * "Reboot Selected": Restarts selected computers during maintenance windows.
* **Benefit**: Streamlines management as the fleet grows without scrolling through dozens of entries.

### 1.3 Detailed Proxmox Error Diagnostics
* **Current Behavior**: Errors during power operations are often summarized as generic failure alerts.
* **Proposed Enhancement**:
  * Surface specific Proxmox task exit strings and QEMU/LXC lock reasons (e.g., `VM is locked (backup)`, `TASK ERROR: timeout waiting for guest agent`, `insufficient cluster memory`).
  * Include a direct "Clear Lock" or "Retry Force Stop" prompt when a VM task is blocked.

---

## 2. 💬 Matrix Chat & Moderation Suite (`/admin` & `/matrix`)

### 2.1 Unified User Moderation Profile Card
* **Proposed Features**:
  * An administrative search bar allowing lookup by Matrix MXID, display name, or student email.
  * Displays an aggregated view:
    * All joined public and private rooms.
    * Current power level per room.
    * Ban / mute status in `matrix_room_settings.json` and Conduit state.
    * Global blacklist / shadow-ban state.
  * **One-Click Full Account Restore**:
    * Unbans the user across all Matrix rooms.
    * Unmutes and resets negative power levels to default (0).
    * Removes entries from `data/blacklist.json` and `data/shadow_bans.json`.
    * Clears temporary IP bans if applicable.
* **Benefit**: Eliminates fragmented moderation steps and ensures users like Long Tran or Drake can be restored completely with a single click.

### 2.2 Automatic Room Re-invitation on Unban
* **Proposed Enhancement**:
  * When an administrator unbans a user from `#general:mitch.pro` or other default channels, optionally send an automatic Matrix invite.
  * **Benefit**: The room automatically reappears in Cinny or Element without requiring the user to manually discover or search the room alias.

### 2.3 Conduit Engine & Federation Diagnostics Widget
* **Proposed Features**:
  * Add a health widget in the `/admin` dashboard displaying:
    * Conduit service status and process uptime.
    * SQLite / RocksDB database file size and compaction status.
    * Inbound and outbound federation queue status.
    * Live active client connections.

---

## 3. 🤖 GitHub PR Automation & Contributor Workflow

### 3.1 AI Safety Scan & Auto-Merge Pipeline for Trusted Contributors
* **Context**: Tyler is configured as a co-owner, and contributors are required to open pull requests rather than pushing directly to `master`.
* **Proposed Enhancement**:
  * Expand `.github/workflows/pr-security-review.yml` to support automated approvals:
    * Check if the PR author is in the approved contributor list (`data/admins.json` co-owners / admins).
    * Run automated static analysis and AI security review for malicious patterns, credential leaks, or unauthorized permission elevations.
    * If unit tests pass and safety scan passes with high confidence, automatically approve and enable auto-merge via a GitHub App or fine-grained repository token.
* **Benefit**: Fast, automated turnaround for standard development without compromising security or requiring manual code sign-off for minor patches.

### 3.2 Branch Rule-Set Verification
* **Proposed Action**:
  * Audit repository rule-sets under Settings → Rules → Rulesets:
    * Target `master`.
    * Ensure "Require a pull request before merging" applies to contributors, while permitting emergency administrator overrides only with signed commits.

---

## 4. 🛡️ Admin Security & Reversible Audit Logs (`/admin/`)

### 4.1 Multi-Criteria Audit Filtering & Search
* **Current Behavior**: The audit log lists recent actions chronologically with single-click revert buttons.
* **Proposed Enhancement**:
  * Add filtering controls:
    * **By Actor**: Filter by `mitch`, `tyler`, or automated system workers.
    * **By Action Type**: Filter by `BAN_USER`, `UNBAN_USER`, `VM_POWER`, `CHANGE_ROLE`, etc.
    * **By Target**: Search by target email or resource ID.
    * **By Date Range**: Quick selectors for `Today`, `Past 7 Days`, `All Time`.

### 4.2 Pre-Revert Diff Preview Modal
* **Proposed Enhancement**:
  * When an administrator clicks "Revert" on an audit item, display a modal showing:
    * The exact state that will be restored (e.g., previous role, ban status, or config value).
    * Confirmation checkbox to avoid accidental reversions of critical configuration.

---

## 5. Priority & Effort Matrix

| Initiative | Impact | Effort | Target Location |
| :--- | :---: | :---: | :--- |
| **Responsive VM Status Polling** | High | Low | `webserver/admin/vms/admin-vms.js` |
| **Unified Matrix User Moderation Card** | High | Medium | `rust/.../routes/matrix.rs`, `webserver/admin/` |
| **Audit Log Filtering & Search** | Medium | Low | `webserver/admin/index.html` |
| **Contributor PR Auto-Merge Automation** | High | Medium | `.github/workflows/` |
| **Conduit Health & Diagnostics Widget** | Medium | Medium | `rust/.../routes/admin/`, `webserver/admin/` |
| **Fleet Bulk Power Controls** | Medium | Medium | `webserver/admin/vms/` |
