# vibebox

`vibebox` is a branch-scoped workspace launcher. Each repo branch gets:

- a persistent Git checkout
- a persistent writable disk cloned from an immutable managed base image
- a stable host-port block so parallel branches do not collide on common dev ports

The current implementation is intentionally small and local-first:

- it prepares the branch workspace
- it checks out or creates the branch in a persistent checkout
- it clones a branch disk from a managed base image into `branch.img`
- it can launch a real VM through built-in `krunkit` + `vmnet-helper` networking or hand off to a configured external launcher

By default, branch disks are expanded to a sparse 32 GiB rootfs even if the imported base image is smaller. Override that with `VIBEBOX_ROOTFS_MIB`.

While developing inside this repo, invoke it with `cargo run -- ...`. After building, the binary is `./target/debug/vibebox`.

## Base Images

Import a Linux image as a managed immutable base:

```bash
cargo run -- base import --name ubuntu-24.04 --image /path/to/ubuntu.img
```

List imported bases:

```bash
cargo run -- base list
```

Imported bases live under:

```text
$VIBEBOX_HOME/base-images/<base-id>/
```

Each base is stored as `base.img` and marked read-only. On macOS, `vibebox` first tries to use APFS `clonefile(2)` semantics when creating copies, then falls back to a normal file copy if a clone is not possible.

## Branch Model

Each `repo + branch` pair is mapped to a stable instance directory under:

```text
$VIBEBOX_HOME/instances/<instance-id>/
```

By default, `VIBEBOX_HOME` resolves to:

```text
~/.local/state/vibebox
```

Inside an instance:

```text
checkout/       persistent repo working tree
vm/branch.img   persistent writable branch disk cloned from the base image
instance.env    persisted instance metadata, including base image and port mappings
```

That means yes: the branch has its own local filesystem block. The `branch.img` file persists until you destroy the instance, but it starts life as a clone of the imported base image rather than as an empty file.
`vibebox` will expand the branch disk file beyond the base image size when needed, and cloud-init plus the guest shell entry path will attempt to grow the root partition/filesystem automatically.

## Usage

Import a base image first:

```bash
cargo run -- base import --name ubuntu-24.04 --image /path/to/ubuntu.img
```

Prepare and enter an existing branch:

```bash
cargo run -- launch --repo git@github.com:org/repo.git --branch main --base ubuntu-24.04
```

Create a new branch from the remote default branch:

```bash
cargo run -- launch --repo git@github.com:org/repo.git --branch feature/x --base ubuntu-24.04 --new-branch
```

Create a new branch from a specific base branch:

```bash
cargo run -- launch --repo git@github.com:org/repo.git --branch feature/x --base ubuntu-24.04 --new-branch --from develop
```

Print the prepared workspace without entering a shell:

```bash
cargo run -- launch --repo git@github.com:org/repo.git --branch main --base ubuntu-24.04 --no-enter
```

Tune VM size explicitly:

```bash
cargo run -- launch --repo git@github.com:org/repo.git --branch main --base ubuntu-24.04 --cpus 6 --memory-mib 12288
```

Set the guest login user, hostname, and SSH key explicitly:

```bash
cargo run -- launch \
  --repo git@github.com:org/repo.git \
  --branch main \
  --base ubuntu-24.04 \
  --cloud-user josh \
  --hostname repo-main \
  --ssh-pubkey ~/.ssh/id_ed25519.pub \
  --ssh-private-key ~/.ssh/id_ed25519
```

Run a first-boot bootstrap script inside the guest user account:

```bash
cargo run -- launch \
  --repo git@github.com:org/repo.git \
  --branch main \
  --base ubuntu-24.04 \
  --cloud-user josh \
  --init-script ./scripts/bootstrap-vm.sh
```

The init script is copied into cloud-init seed data, runs once per instance as the guest user, and can use `sudo` for system package installs. Output is written inside the guest to `/var/log/vibebox-init.log`.

Inspect or remove a branch instance:

```bash
cargo run -- status --repo git@github.com:org/repo.git --branch main
cargo run -- destroy --repo git@github.com:org/repo.git --branch main
```

## Port Mapping

The launcher reserves a deterministic host port block and maps common guest dev ports:

- `22`
- `3000`
- `5173`
- `5432`
- `6379`
- `8000`
- `8080`
- `8081`

The first free block is persisted in `instance.env`, so the same branch keeps the same host ports over time.

## Built-In VM Runtime

If `krunkit` and `vmnet-helper` are installed, `vibebox launch` will boot the guest, wait for SSH, and connect automatically.

The built-in runtime launches:

- `krunkit` with the branch disk attached as `virtio-blk`
- `vmnet-client` to provide macOS shared networking
- a per-instance cloud-init `CIDATA` seed ISO as a second `virtio-blk` device
- the host checkout as a `virtio-fs` share with mount tag `workspace`
- a guest console log at `runtime/console.log`
- SSH as the shell/control plane
- SSH `-L` tunnels for deterministic branch-local host ports

Cloud-init is enabled by default for VM launches. `vibebox` will:

- auto-detect `~/.ssh/id_ed25519.pub`, `id_ecdsa.pub`, `id_rsa.pub`, or `authorized_keys`
- create a sudo-capable guest user, defaulting to your current host username
- set the guest hostname from the instance id unless you override it
- write a `network-config` file that assigns a deterministic static guest IP
- auto-mount the `virtio-fs` share at `/workspace`
- create `~/workspace` as a symlink to `/workspace`
- install and start `avahi-daemon` so the guest hostname is advertised over mDNS as `<hostname>.local`
- write `cloud-init/files/user-data` and `cloud-init/files/meta-data`
- build `cloud-init/seed.iso` with volume label `CIDATA`

Disable seed generation with `--no-cloud-init` if your base image is already preconfigured. Use `--shell` if you explicitly want a local shell instead of a VM; VM launch is the default.

Inside the guest, the repo should already be available at `/workspace`, and the cloud-init user should also have `~/workspace` pointing at it. `vibebox` should SSH there automatically once the guest is up.

On macOS, the built-in VM path plus `avahi-daemon` means you can typically open guest-hosted dev servers in a browser via the guest hostname, for example `http://repo-main.local:3000`, as long as the service is listening on the guest network interface.

Your base image therefore needs to be:

- Linux
- EFI bootable
- compatible with virtio block and virtio-fs
- running `sshd`

For built-in networking on macOS, install `vmnet-helper`:

- repo: https://github.com/nirs/vmnet-helper
- install script from the README: `curl -fsSL https://raw.githubusercontent.com/nirs/vmnet-helper/main/install.sh | sudo bash`

## External Handoff

Set `VIBEBOX_VM_LAUNCHER` to an executable that knows how to translate the instance metadata into your local libkrun invocation.

The launcher receives:

- `VIBEBOX_INSTANCE`
- `VIBEBOX_REPO`
- `VIBEBOX_BRANCH`
- `VIBEBOX_CHECKOUT`
- `VIBEBOX_BASE_IMAGE`
- `VIBEBOX_BASE_IMAGE_ID`
- `VIBEBOX_ROOTFS`
- `VIBEBOX_ROOTFS_MB`
- `VIBEBOX_CPUS`
- `VIBEBOX_MEMORY_MIB`
- `VIBEBOX_CLOUD_INIT_IMAGE`
- `VIBEBOX_CLOUD_INIT_USER`
- `VIBEBOX_HOSTNAME`
- `VIBEBOX_GUEST_IP`
- `VIBEBOX_GUEST_GATEWAY`
- `VIBEBOX_GUEST_MAC`
- `VIBEBOX_INTERFACE_ID`
- `VIBEBOX_SSH_PRIVATE_KEY`
- `VIBEBOX_PORTS` as `host:guest,host:guest,...`

If neither `VIBEBOX_VM_LAUNCHER` nor the built-in `krunkit` + `vmnet-helper` path is available, `vibebox launch` will fail unless you explicitly pass `--shell`.
