# Quick start

```sh
git clone https://github.com/joshuaswarren/omalight.git && cd omalight
```

The engine runs on the machine that holds the photos. That can be a Mac Studio or a Linux box. The window runs on the Omarchy laptop, or on the same machine. The first start creates a secret on its own. You confirm a six-digit code once. After that the laptop reconnects without asking.

A Tailscale name works the same as any other hostname.

## Engine on macOS

You need the Xcode command line tools and Rust 1.90 or newer.

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup default stable
rustc --version
```

`rustc --version` has to report 1.90 or newer. LightCraft does not build on older compilers.

From a checkout of this repo:

```sh
cargo build --release -p omalight
./packaging/install.sh
omalight-engine install-service
```

Leave `LIGHTCRAFT_GPU` unset. The engine uses Metal through wgpu.

`install-service` writes `~/Library/LaunchAgents/com.omalight.engine.plist` and loads it, so the engine starts at login. It listens on port 7421.

## Engine on Linux

```sh
sudo pacman -S --needed rust wayland libxkbcommon mesa pkgconf
rustc --version
```

If that `rustc` is older than 1.90, install rustup and use stable instead of the distro package.

From a checkout of this repo:

```sh
cargo build --release -p omalight
./packaging/install.sh
omalight-engine install-service
```

Leave `LIGHTCRAFT_GPU` unset. The engine uses Vulkan when a GPU is present.

`install-service` writes `~/.config/systemd/user/omalight-engine.service` and enables it for this user.

## On the laptop

```sh
sudo pacman -S --needed rust wayland libxkbcommon mesa pkgconf
cargo build --release -p omalight
./packaging/install.sh
omalight
```

Pick the engine in the list. The engine prints a six-digit code, and shows a desktop notification where the machine has one. The window shows the same code. Confirm once.

Open a Sony ARW. The path is opened by the engine, so the file has to be on the engine machine:

```sh
omalight /path/on/the/engine/frame.ARW
```

If you already have SSH to the engine:

```sh
omalight pair studio.local
```

`studio.local` is the engine's hostname. SSH is tried first. If it does not work, you get the same code to confirm.

## If the engine is not in the list

The engine process has to be running, and TCP port 7421 has to be allowed on that machine. Then:

```sh
omalight pair studio.local
```

To pair that laptop again, on the engine:

```sh
omalight-engine devices
omalight-engine revoke laptop
```

`laptop` is the name printed by `devices`. It defaults to the laptop's hostname. Then run `omalight pair studio.local` again.

## Check

```sh
omalight doctor
omalight doctor --connect studio.local
```

This prints the config directory, the secret file, paired devices, theme detection, the GPU switch, and whether the engine answers. A failed check has a `fix:` line under it.
