# Host Artifact, Clipboard, and Guest Skills Plan

## Problem

Agents running inside a `yolobox` guest can generate HTML or other artifacts in a shared path such as `/workspace/.artifacts/...`, but they cannot directly open those files in a browser on the macOS host. Guest-side commands like `open` or `xdg-open` either target the Linux guest or fail because no browser is configured there.

Clipboard access has the same shape. A guest process cannot reliably access the macOS clipboard through Linux clipboard APIs, and exposing unrestricted clipboard reads to the guest would be too broad and unsafe.

We want a narrow, explicit way for a guest process to request "open this shared file on the host" without creating a general host-command escape hatch.

## Goals

- Let guest-side tools request opening a host-visible artifact.
- Let guest-side tools request importing the current host clipboard image into a shared file.
- Keep the security boundary narrow and auditable.
- Give agents a clear, environment-specific workflow.
- Reuse existing `yolobox` shared-path behavior instead of inventing a separate artifact transport.

## Non-Goals

- Arbitrary host command execution from the guest.
- Opening files that are not in allowlisted shared paths.
- Unrestricted guest access to the current host clipboard.
- Making guest-local paths automatically visible on the host.

## Proposed Shape

Implement three cooperating pieces:

1. Guest helper package: `yolobox-tools`
2. Host-side request handling in the `yolobox` launcher
3. Agent instructions that explain the environment and preferred workflow
4. A shared `/skills` mount in the guest for environment-specific guidance

## Guest Helper

Install a tiny helper package in the guest, tentatively named `yolobox-tools`.

Initial command surface:

- `yolobox-open <guest-path>`
- `yolobox-paste-image <guest-path>`

Responsibilities:

- Accept a guest path such as `/workspace/.artifacts/visual-123/index.html`
- Confirm the path is inside an allowlisted shared location
- Emit a structured request to the host-side launcher
- Avoid attempting GUI actions locally in the guest

For `yolobox-paste-image`, the guest path is the destination path where the host clipboard image should be written as a file.

This helper is a request shim, not a browser launcher or clipboard implementation.

Possible future commands:

- `yolobox-reveal <guest-path>`
- `yolobox-print-host-path <guest-path>`

## Host-Side Handling

The host-side `yolobox` launcher remains the authority.

Responsibilities:

- Receive requests from the guest
- Validate that the requested path is inside an allowlisted shared directory
- Translate the guest path to the corresponding host path
- For open requests, confirm the target exists and run macOS `open` only after validation succeeds
- For clipboard-image requests, obtain the current macOS clipboard image and write it to the validated host path only after user approval

Important constraints:

- The host listener must support narrow verbs such as "open this file" or "materialize the current clipboard image here", not "run this host command".
- Clipboard-image import should not be automatic by default because a malicious agent could otherwise poll and track the user's clipboard contents.

## Control Channel Options

There are a few workable transport options between guest helper and host launcher.

### Option 1: Shared Request Directory

The guest writes a request file into a shared control directory. The host watches or polls that directory.

Pros:

- Easy to inspect and debug
- Fits the existing shared-files model
- Does not require a long-lived RPC stack

Cons:

- Requires file cleanup and request lifecycle handling

### Option 2: Stdout Marker

The guest helper prints a machine-readable marker and the host launcher watches terminal output.

Pros:

- Very small v1
- No extra on-disk protocol

Cons:

- More fragile
- Tied to interactive launcher output handling

### Option 3: Socket / RPC Channel

Use a dedicated Unix socket, vsock, or another RPC path.

Pros:

- Clean protocol boundary
- Extensible

Cons:

- More engineering than needed for v1

## Recommended V1

Start with:

- `yolobox-tools` in the guest
- A dedicated shared `/yolobox` mount as the control and integration surface
- Two supported verbs:
  - open a shared artifact on the host
  - import the current host clipboard image to a shared destination file
- Two allowlisted areas:
  - `/workspace/.artifacts/` for generated outputs
  - `/yolobox/inputs/` for imported inputs such as clipboard images

This keeps repo content in `/workspace` and control/runtime integration data out of the git checkout.

## `/yolobox` Mount Layout

The guest should see a dedicated shared mount at `/yolobox`.

Recommended host-side backing path:

- `~/.local/state/yolobox/instances/<id>/runtime/yolobox/`

Recommended guest layout:

- `/yolobox/requests/`
- `/yolobox/responses/`
- `/yolobox/inputs/`
- `/yolobox/scripts/`
- `/yolobox/skills/`

This keeps the git checkout clean while giving the guest a stable place for integration features.

## Suggested Workflow

### Open Artifact

1. Agent generates an artifact under `/workspace/.artifacts/...`
2. Agent runs `yolobox-open /workspace/.artifacts/.../index.html`
3. Guest helper writes an open request
4. Host launcher validates and maps the path to the host checkout
5. Host runs macOS `open <host-path>`

### Paste Clipboard Image

1. Agent runs `yolobox-paste-image /yolobox/inputs/clipboard.png`
2. Guest helper writes a clipboard-image request
3. Host launcher validates the destination path
4. Host launcher asks the user for confirmation
5. If approved, host reads the current macOS clipboard image
6. Host writes the image to the mapped host path as a file
7. Guest continues using the imported file from the shared path

If validation fails, the host should reject the request clearly and the guest helper should surface that failure.

## Validation Rules

Host-side validation should be strict.

Minimum checks:

- Requested path must be absolute
- Requested path must resolve under an allowlisted shared root
- Requested path must not escape the root through `..` or symlink resolution

Verb-specific checks:

- For `open`:
  - Requested file must exist
- For `paste-image`:
  - Destination parent directory must exist or be creatable under the allowlisted root
  - Destination should be a file path, not a directory
  - Host clipboard must currently contain an image

Optional additional checks:

- Restrict to file extensions such as `.html`, `.htm`, `.svg`, `.png`, `.jpg`, `.pdf`
- Reject directories for v1

## Clipboard Confirmation UX

Clipboard-image requests should require per-request user approval in v1.

Reason:

- Automatic clipboard access would let a malicious or buggy agent repeatedly read and track the current host clipboard.

Recommended host behavior:

- Show a simple native macOS confirmation dialog for each clipboard-image request
- Include the instance name and destination path in the message
- Time out or reject if the user does not approve

Recommended implementation for v1:

- Use `osascript` from the host-side listener to display the confirmation dialog

Reason:

- It is the smallest practical way to show a native macOS prompt from Rust without building a full GUI layer first

## Agent Guidance

Agents need explicit environment-specific instructions or they will keep trying `open` / `xdg-open`.

Minimum guidance block:

- You are running inside a Linux guest in `yolobox`
- The host is macOS
- GUI apps do not open from inside the guest
- To expose files to the host, write them under `/workspace`
- Preferred output path: `/workspace/.artifacts/`
- Preferred imported-input path: `/yolobox/inputs/`
- To request opening a host-visible artifact, run `yolobox-open <guest-path>`
- To request importing the current host clipboard image, run `yolobox-paste-image <guest-path>`
- Do not use `open`, `xdg-open`, or other guest GUI launchers for host viewing
- Do not use Linux clipboard tools to access the host clipboard

## Shared Skills Mount

It should be possible to mount a host-managed skills directory into the guest at `/yolobox/skills`.

This can be done either:

- explicitly with a normal `--share` mount, or
- automatically as part of `yolobox`'s built-in agent integration

Example shape:

- host path: `~/.local/state/yolobox/instances/<id>/runtime/yolobox/skills/`
- guest path: `/yolobox/skills`

Recommended properties:

- Prefer read-only if the runtime can support it cleanly
- Keep the host copy as the source of truth
- Organize by agent plus a shared common area

Suggested directory layout:

- `/yolobox/skills/common/`
- `/yolobox/skills/codex/`
- `/yolobox/skills/claude/`

## Skill Population

Default skills should be populated when the instance is created.

Recommended behavior:

- Create the `/yolobox/scripts/` tree during instance creation
- Create the `/yolobox/skills/` tree during instance creation
- Populate `/yolobox/scripts/` with default helper commands such as `yolobox-open` and `yolobox-paste-image`
- Populate it with default environment guidance for supported agents
- Treat the generated defaults as part of the per-instance runtime state

This gives every new instance a baseline set of environment instructions without requiring manual setup.

## Suggested Skill Content

Each agent should have a small `yolobox` environment skill that explains the guest/host boundary and the helper commands.

Minimum topics to cover:

- You are running inside a Linux guest in `yolobox`
- The host is macOS
- Shared files are available through `/workspace` and any configured shared mounts
- `yolobox-tools` exists for narrow host interactions
- Use `/yolobox/scripts/yolobox-open <path>` to open a host-visible artifact
- Use `/yolobox/scripts/yolobox-paste-image <path>` to request importing the current host clipboard image
- Guest GUI launchers such as `open` or `xdg-open` are not the right mechanism for host viewing
- Linux clipboard tooling does not provide direct access to the macOS host clipboard
- The instance has an mDNS hostname such as `<instance>.local`
- Host users should access guest services via the mDNS hostname and port, not via the guest's `localhost`
- `localhost:<port>` inside the guest is not the same as `localhost:<port>` on the host

## Network Guidance for Skills

The skills should explicitly teach the agent how guest networking appears from the host.

Minimum guidance:

- Services started in the guest are reachable from the host by mDNS hostname, for example `http://<instance>.local:3000`
- The host should not be told to visit `http://localhost:<port>` unless there is an explicit host-side port forward
- When giving the user a URL for a guest service, prefer the mDNS hostname form
- If the instance name or hostname is known, include it directly in instructions to the user

This prevents a common failure mode where an agent starts a dev server inside the guest and then incorrectly tells the user to open `localhost`.

## Why This Shape

This design gives us:

- Good UX for agents and users
- A narrow trust boundary
- A clear audit surface
- Room to add more host actions later without exposing arbitrary host execution
- Explicit protection around sensitive clipboard access

## Open Questions

- Should the request channel be per-instance under instance state, or a conventional path in `/workspace`?
- Should v1 support only files, or also "reveal in Finder"?
- Should allowlisted `open` requests be automatic, or should some users prefer confirmation there too?
- Should the host report completion back to the guest helper, or is fire-and-forget sufficient for `open` while clipboard import expects an explicit success/failure result?
