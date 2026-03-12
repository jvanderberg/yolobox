# Repo Rules

## Launcher Verification

For any change that affects the `yolobox` launcher, guest bootstrap, SSH entry path, shell profile, host bridge, or other runtime behavior that depends on the live dev binary:

- Do not treat `cargo test` as proof that the interactive launcher behavior is live.
- Do not treat `cargo run` as the verification target.
- Build with `cargo build` and run `target/debug/yolobox` directly.
- If observed runtime behavior conflicts with the source change, run `cargo clean` and rebuild before reasoning further.
- If the change affects launch-time/bootstrap state, restart the instance before testing.
- Verify from a host-side artifact, process state, or runtime file when possible; do not rely only on terminal output that can be overwritten.

In short: source change -> `cargo build` -> `target/debug/yolobox` -> restart if needed -> verify against live runtime state.
