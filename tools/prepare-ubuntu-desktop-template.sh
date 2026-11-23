#!/usr/bin/env bash
set -euo pipefail

# Run on the Proxmox host. Creates only the explicitly unused template VMID.
TEMPLATE_ID="${1:-9010}"
[[ "$TEMPLATE_ID" =~ ^9[0-9]{3}$ ]] || { echo 'Use an unused template VMID between 9000 and 9999.'; exit 1; }
if qm config "$TEMPLATE_ID" >/dev/null 2>&1; then
  echo "VM $TEMPLATE_ID already exists; refusing to overwrite it."
  exit 1
fi

# Student bridge / SDN VNet the template NIC attaches to (same value as the
# portal's PROXMOX_VM_BRIDGE). Required: there is no default.
STUDENT_BRIDGE="${PROXMOX_VM_BRIDGE:-}"
[[ -n "$STUDENT_BRIDGE" ]] || { echo 'Set PROXMOX_VM_BRIDGE to the student bridge (e.g. stunet).'; exit 1; }

BUILD_DIR="/var/lib/vz/desktop-template-build"
mkdir -p "$BUILD_DIR"
chmod 700 "$BUILD_DIR"
cd "$BUILD_DIR"
IMAGE='ubuntu-24.04-server-cloudimg-amd64.img'
BASE='https://cloud-images.ubuntu.com/releases/noble/release-20260826'
curl --fail --location --retry 3 --output "$IMAGE" "$BASE/$IMAGE"
curl --fail --location --retry 3 --output SHA256SUMS "$BASE/SHA256SUMS"
awk -v file="$IMAGE" '$2 == "*"file || $2 == file {print}' SHA256SUMS | sha256sum --check --status
echo 'Ubuntu image SHA256 verified.'
KEY="$BUILD_DIR/build-access"
ssh-keygen -q -t ed25519 -N '' -C 'temporary-desktop-template-build' -f "$KEY"
qm create "$TEMPLATE_ID" --name ubuntu-desktop-24-04-template --memory 8192 --cores 4 --cpu x86-64-v2-AES --net0 "virtio,bridge=$STUDENT_BRIDGE,firewall=1" --scsihw virtio-scsi-single --agent 1 --vga std,memory=64 --serial0 socket --ostype l26 --pool sandboxes
qm importdisk "$TEMPLATE_ID" "$BUILD_DIR/$IMAGE" local-lvm
qm set "$TEMPLATE_ID" --scsi0 "local-lvm:vm-$TEMPLATE_ID-disk-0,discard=on,iothread=1" --ide2 local-lvm:cloudinit --boot order=scsi0 --ciuser ubuntu --sshkeys "$KEY.pub" --ipconfig0 ip=dhcp --nameserver 1.1.1.1
qm resize "$TEMPLATE_ID" scsi0 40G
qm start "$TEMPLATE_ID"
echo "Template build VM $TEMPLATE_ID started. Install desktop packages, verify GDM and guest agent, then sanitize before qm template."
