# Website virtualization removal

Tylerium does not integrate with Proxmox or another hypervisor. VM/LXC labs,
provisioning, consoles, management, custom ISO uploads, capacity/usage workers,
owner-container access, VM restrictions, permission grants, and VM database
models have been removed from application source. The corresponding frontend
pages, links, controls, environment settings, and VM-only libraries are removed.

The router rejects the complete former route families with HTTP 404 before auth,
proxy handling, or static serving. Rotating URL aliases resolve before this check.
The retirement integration test exercises GET/POST/DELETE, with and without a
session, and checks that a fresh database creates no VM tables.

This change performs no hypervisor operations. No guest, container, hypervisor
configuration, existing production database, or backup is deleted or modified.
The new deployment uses separate storage. Do not use application retirement as
permission to clean up real machines or old production records.

All non-VM application features remain in scope, including the economy, shop,
casino, cosmetics, games, profiles, messaging, community, and administration.
