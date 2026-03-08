# Manual Install

Use this path if you do not want to run `scripts/setup.sh`.

## Prerequisites

`yolobox` currently expects:

- macOS
- an APFS volume for `YOLOBOX_HOME`
- an SSH key at `~/.ssh/id_ed25519`, `~/.ssh/id_ecdsa`, or `~/.ssh/id_rsa`

Install the host dependencies:

```bash
# VM runtime
brew tap slp/krunkit
brew install krunkit

# Networking
curl -fsSL https://raw.githubusercontent.com/nirs/vmnet-helper/main/install.sh | sudo bash

# Image conversion
brew install qemu
```

If you do not already have an SSH key:

```bash
ssh-keygen -t ed25519
```

## Build And Install

From a local checkout:

```bash
cargo build --release
install -d ~/.local/bin
install -m 0755 ./target/release/yolobox ~/.local/bin/yolobox
```

Make sure `~/.local/bin` is on your `PATH`.

## Import A Base Image

Any Linux image that supports EFI boot, `cloud-init`, `sshd`, `virtio-blk`, and `virtio-fs` will work. Ubuntu cloud images are a good default:

```bash
curl -LO https://cloud-images.ubuntu.com/jammy/current/jammy-server-cloudimg-arm64.img
qemu-img convert -f qcow2 -O raw jammy-server-cloudimg-arm64.img ubuntu-jammy-arm64.raw
yolobox base import --name ubuntu --image ./ubuntu-jammy-arm64.raw
```

Skip the `qemu-img` step if your download is already a raw image.

## Optional: Prepare A Dev Base

To create a pre-provisioned `ubuntu-dev` base:

```bash
yolobox --name setup-ubuntu-dev --base ubuntu --init-script ./scripts/bootstrap-vm.sh
yolobox base capture --name ubuntu-dev --instance setup-ubuntu-dev
yolobox destroy --name setup-ubuntu-dev --yes
```

## Verify

```bash
yolobox doctor
```
