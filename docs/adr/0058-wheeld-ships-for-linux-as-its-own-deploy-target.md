# ADR-0058: wheeld ships for Linux as its own deploy target

On Linux only a daemon runs. wheeld and the `wheel` CLI build for `x86_64-unknown-linux-gnu`; no client is built for Linux. Each `target_os = "macos"` gate in wheeld has a Linux branch or states that the feature does not exist on Linux.

The always-on agent host is a deploy target of its own. The Mac cross-compiles both binaries with `cargo zigbuild --target x86_64-unknown-linux-gnu.2.41` and copies them to `~/.local/share/wheel/server/versions/<source hash>/` on the host; the host needs nothing but the binaries. wheeld runs there as a `systemd --user` unit whose program is `~/.local/share/wheel/server/current/wheeld`, with its remote listener on the host's Tailscale address. Devices pair with it by its pairing code; `wheel remote pair --qr` prints the code as a terminal QR on the host itself.

Deploys follow ADR-0020's split. `make deploy-linux` stages a version on the host without running it, and `make deploy-client` runs it last. `make restart-server-linux` moves `current` to the staged version and restarts the unit, and `make restart-server` runs it after the Mac's restart. An unreachable host never fails the Mac's step: the step reports the host as behind and exits zero.

This narrows ADR-0013 for the Mac only: Wheel.app still carries every program it runs, and the Linux wheeld is a separate install the Mac's deploy places on the host.
