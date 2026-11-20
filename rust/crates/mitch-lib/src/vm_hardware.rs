//! Virtual hardware a member can choose for a Preset OS or Custom OS
//! computer, and the paid VirGL entitlement that limits one of those choices.
//!
//! Everything except the display is free to choose: firmware, machine type,
//! TPM, network card and disk bus exist so people can get unusual operating
//! systems to boot. The `virtio-gl` display (VirGL, GPU-accelerated 3D) uses
//! the shared host GPU, so it is a one-time purchase per account
//! ([`VIRGL_FEATURE`], stored in `vm_entitlements`). A member without it can
//! use every other display, including plain `virtio`.
//!
//! [`HardwareOptions::from_json`] is the single validation point: every field
//! is checked against a fixed allowlist, and unknown values are rejected
//! rather than passed to Proxmox. The server must additionally refuse
//! [`HardwareOptions::needs_virgl`] unless [`has_virgl`] is true; the power-on
//! path downgrades `virtio-gl` for accounts without it as a second check
//! (`ProxmoxDesktopService::ensure_optimized_vm_config`). See
//! `docs/computer-portal.md`.

use crate::auth::{is_owner_email, normalize_email};
use crate::data::DataStore;
use serde_json::Value;

/// `vm_entitlements.feature` for the VirGL display.
pub const VIRGL_FEATURE: &str = "virgl";
/// One-time price of VirGL, in MitchCoins. It is permanent and never
/// refunded, so it cannot be bought and sold back for coins.
pub const VIRGL_COST_COINS: f64 = 1500.0;

/// Whether the account may use the `virtio-gl` display. The site owner always
/// may (their own hardware).
pub fn has_virgl(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    if is_owner_email(store, email) {
        return true;
    }
    has_entitlement(store, email, VIRGL_FEATURE)
}

/// Whether a purchase row exists (ignores the owner shortcut).
pub fn has_entitlement(store: &DataStore, email: &str, feature: &str) -> bool {
    let conn = store.conn();
    conn.query_row(
        "SELECT 1 FROM vm_entitlements WHERE email = ?1 AND feature = ?2",
        rusqlite::params![normalize_email(email), feature],
        |_| Ok(()),
    )
    .is_ok()
}

/// Claims an entitlement. Returns `true` only for the call that created the
/// row, so two simultaneous purchases can never both charge: the caller
/// claims first, debits only if this returned `true`, and calls [`revoke`] if
/// the debit fails.
pub fn claim(store: &DataStore, email: &str, feature: &str, now: i64) -> bool {
    let conn = store.conn();
    conn.execute(
        "INSERT OR IGNORE INTO vm_entitlements (email, feature, granted_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![normalize_email(email), feature, now],
    )
    .unwrap_or(0)
        > 0
}

/// Removes an entitlement (used to undo a claim whose payment failed).
pub fn revoke(store: &DataStore, email: &str, feature: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM vm_entitlements WHERE email = ?1 AND feature = ?2",
        rusqlite::params![normalize_email(email), feature],
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Std,
    Qxl,
    Vmware,
    Virtio,
    VirtioGl,
}

impl Display {
    /// Proxmox `vga` value.
    pub fn vga(self) -> &'static str {
        match self {
            Display::Std => "std,memory=128",
            Display::Qxl => "qxl,memory=128",
            Display::Vmware => "vmware,memory=128",
            Display::Virtio => "virtio,memory=128",
            // virtio-gl takes no `memory` suboption.
            Display::VirtioGl => "virtio-gl",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Firmware {
    Uefi,
    Bios,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    Q35,
    I440fx,
}

impl Machine {
    /// Proxmox `machine` value (`pc` is the i440fx chipset).
    pub fn value(self) -> &'static str {
        match self {
            Machine::Q35 => "q35",
            Machine::I440fx => "pc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nic {
    Virtio,
    E1000e,
    E1000,
    Rtl8139,
}

impl Nic {
    /// Proxmox network model name.
    pub fn model(self) -> &'static str {
        match self {
            Nic::Virtio => "virtio",
            Nic::E1000e => "e1000e",
            Nic::E1000 => "e1000",
            Nic::Rtl8139 => "rtl8139",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskBus {
    Scsi,
    Sata,
}

impl DiskBus {
    /// Proxmox key of the primary disk.
    pub fn key(self) -> &'static str {
        match self {
            DiskBus::Scsi => "scsi0",
            DiskBus::Sata => "sata0",
        }
    }
}

/// Hardware choices for a new computer. `None` means "use the default for
/// this OS", so an empty value behaves exactly as before this feature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HardwareOptions {
    pub display: Option<Display>,
    pub firmware: Option<Firmware>,
    pub machine: Option<Machine>,
    pub tpm: Option<bool>,
    pub nic: Option<Nic>,
    pub disk_bus: Option<DiskBus>,
}

impl HardwareOptions {
    /// True when this selection needs the paid VirGL display.
    pub fn needs_virgl(&self) -> bool {
        self.display == Some(Display::VirtioGl)
    }

    /// Parses the `hardware` object of a create request. A missing/null value
    /// is all defaults; `"default"`/empty strings are also defaults. Any
    /// other unrecognized value is an error.
    pub fn from_json(value: Option<&Value>) -> Result<Self, String> {
        let Some(Value::Object(map)) = value.filter(|v| !v.is_null()) else {
            if value.is_some_and(|v| !v.is_null()) {
                return Err("Invalid hardware options.".to_string());
            }
            return Ok(Self::default());
        };
        fn pick<'a>(
            map: &'a serde_json::Map<String, Value>,
            key: &str,
        ) -> Result<Option<&'a str>, String> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) if s.is_empty() || s == "default" => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.as_str())),
                Some(_) => Err(format!("Invalid hardware option: {key}.")),
            }
        }
        fn bad(key: &str) -> String {
            format!("Invalid hardware option: {key}.")
        }
        let display = match pick(map, "display")? {
            None => None,
            Some("std") => Some(Display::Std),
            Some("qxl") => Some(Display::Qxl),
            Some("vmware") => Some(Display::Vmware),
            Some("virtio") => Some(Display::Virtio),
            Some("virtio-gl") => Some(Display::VirtioGl),
            Some(_) => return Err(bad("display")),
        };
        let firmware = match pick(map, "firmware")? {
            None => None,
            Some("uefi") => Some(Firmware::Uefi),
            Some("bios") => Some(Firmware::Bios),
            Some(_) => return Err(bad("firmware")),
        };
        let machine = match pick(map, "machine")? {
            None => None,
            Some("q35") => Some(Machine::Q35),
            Some("i440fx") => Some(Machine::I440fx),
            Some(_) => return Err(bad("machine")),
        };
        let tpm = match map.get("tpm") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::String(s)) if s.is_empty() || s == "default" => None,
            Some(Value::String(s)) if s == "on" => Some(true),
            Some(Value::String(s)) if s == "off" => Some(false),
            Some(_) => return Err(bad("tpm")),
        };
        let nic = match pick(map, "nic")? {
            None => None,
            Some("virtio") => Some(Nic::Virtio),
            Some("e1000e") => Some(Nic::E1000e),
            Some("e1000") => Some(Nic::E1000),
            Some("rtl8139") => Some(Nic::Rtl8139),
            Some(_) => return Err(bad("nic")),
        };
        let disk_bus = match pick(map, "diskBus")? {
            None => None,
            Some("scsi") => Some(DiskBus::Scsi),
            Some("sata") => Some(DiskBus::Sata),
            Some(_) => return Err(bad("diskBus")),
        };
        Ok(Self {
            display,
            firmware,
            machine,
            tpm,
            nic,
            disk_bus,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-vm-hw-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn missing_or_default_values_mean_defaults() {
        assert_eq!(
            HardwareOptions::from_json(None).unwrap(),
            HardwareOptions::default()
        );
        let opts = HardwareOptions::from_json(Some(
            &json!({"display": "default", "firmware": "", "tpm": null}),
        ))
        .unwrap();
        assert_eq!(opts, HardwareOptions::default());
        assert!(!opts.needs_virgl());
    }

    #[test]
    fn parses_every_field() {
        let opts = HardwareOptions::from_json(Some(&json!({
            "display": "virtio-gl", "firmware": "bios", "machine": "i440fx",
            "tpm": "off", "nic": "rtl8139", "diskBus": "sata",
        })))
        .unwrap();
        assert!(opts.needs_virgl());
        assert_eq!(opts.firmware, Some(Firmware::Bios));
        assert_eq!(opts.machine, Some(Machine::I440fx));
        assert_eq!(opts.tpm, Some(false));
        assert_eq!(opts.nic, Some(Nic::Rtl8139));
        assert_eq!(opts.disk_bus, Some(DiskBus::Sata));
        assert_eq!(Display::VirtioGl.vga(), "virtio-gl");
    }

    #[test]
    fn rejects_unknown_values_instead_of_forwarding_them() {
        for bad in [
            json!({"display": "std,memory=999999"}),
            json!({"nic": "virtio,bridge=vmbr0"}),
            json!({"machine": "q35,viommu=on"}),
            json!({"diskBus": "ide"}),
            json!({"tpm": 3}),
            json!({"firmware": ["uefi"]}),
        ] {
            assert!(HardwareOptions::from_json(Some(&bad)).is_err(), "{bad}");
        }
        assert!(HardwareOptions::from_json(Some(&json!("uefi"))).is_err());
    }

    #[test]
    fn only_the_virtio_gl_display_needs_the_entitlement() {
        for display in ["std", "qxl", "vmware", "virtio"] {
            let opts = HardwareOptions::from_json(Some(&json!({ "display": display }))).unwrap();
            assert!(!opts.needs_virgl(), "{display}");
        }
    }

    #[test]
    fn claim_succeeds_once_and_can_be_undone() {
        let store = temp_store("claim");
        assert!(!has_virgl(&store, "student@example.com"));
        assert!(claim(&store, "Student@Example.com", VIRGL_FEATURE, 1));
        // A second (concurrent or repeated) purchase must not claim again.
        assert!(!claim(&store, "student@example.com", VIRGL_FEATURE, 2));
        assert!(has_virgl(&store, "student@example.com"));
        revoke(&store, "student@example.com", VIRGL_FEATURE);
        assert!(!has_virgl(&store, "student@example.com"));
    }
}
