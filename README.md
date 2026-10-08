# Omalight

Photo window for Omarchy. A daemon runs LightCraft next to the library, on a Mac Studio (Metal) or a Linux GPU box (Vulkan). The window sends edits and receives JPEG previews. If that host is down, the window starts a local engine.

Rust throughout. Omalight is built on [LightCraft](https://github.com/storytold/lightcraft) by storytold, pinned as a git dependency. See `docs/ARCHITECTURE.md`. The short version of the install is `docs/QUICKSTART.md`. The license is MIT OR Apache-2.0.

## Quick start

```sh
git clone https://github.com/joshuaswarren/omalight.git && cd omalight
```

The engine runs on the machine that holds the photos. The window runs on the Omarchy laptop, or on the same machine. The first start creates a secret. You confirm a six-digit code once. After that the laptop reconnects on its own.

A Tailscale name works the same as any other hostname.

### Engine on macOS

Xcode command line tools, and Rust 1.90 or newer:

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup default stable
rustc --version
```

From a checkout of this repo:

```sh
cargo build --release -p omalight
./packaging/install.sh
omalight-engine install-service
```

Leave `LIGHTCRAFT_GPU` unset. The engine uses Metal through wgpu. `install-service` writes `~/Library/LaunchAgents/com.omalight.engine.plist` and loads it. The engine listens on port 7421.

### Engine on Linux

```sh
sudo pacman -S --needed rust wayland libxkbcommon mesa pkgconf
rustc --version
```

If that compiler is older than 1.90, install rustup and use stable.

```sh
cargo build --release -p omalight
./packaging/install.sh
omalight-engine install-service
```

Leave `LIGHTCRAFT_GPU` unset so a GPU uses Vulkan. `install-service` writes a systemd user unit and enables it.

### On the laptop

```sh
sudo pacman -S --needed rust wayland libxkbcommon mesa pkgconf
cargo build --release -p omalight
./packaging/install.sh
omalight
```

Pick the engine in the list. Confirm the code it shows, which is the same code printed by the engine. Then open a file that exists on the engine:

```sh
omalight /path/on/the/engine/frame.ARW
```

If you already have SSH to the engine host, `omalight pair studio.local` fetches the credential that way and falls back to the code if SSH is not available. `studio.local` is that machine's hostname.

If the laptop cannot see the engine, check that `omalight-engine` is running and that TCP port 7421 is allowed, or run `omalight pair <engine-host>`. To pair again: `omalight-engine revoke NAME` on the engine, then `omalight pair <engine-host>`.

```sh
omalight doctor
omalight doctor --connect studio.local
```

## Config

`config.toml` lives in the config directory: `~/Library/Application Support/omalight` on macOS, `$XDG_CONFIG_HOME/omalight` or `~/.config/omalight` on Linux. `OMALIGHT_CONFIG_DIR` overrides it.

```toml
mode = "auto"   # auto | local | remote
host = "studio.local"
port = 7421

[ai]
base_url = "http://127.0.0.1:8080/v1"
model = "local"
api_key_env = "OMALIGHT_AI_KEY"
```

The engine writes `secret` there on first start, mode 0600. Paired devices are listed with `omalight-engine devices`. The laptop stores its credential in `paired.toml` in its own config directory.

`LIGHTCRAFT_GPU=0` keeps rendering on the CPU. Leave it unset on a machine with Metal or Vulkan.

Headless, after pairing:

```sh
omalight --headless --connect 127.0.0.1:7421 \
  --open photo.dng --set light.exposure=0.3 --set wb.temp=5200 \
  --export /tmp/out.jpg --latency-json /tmp/latency.json
```

## Build

Rust 1.90 or newer. Wayland and X11 client libraries, used by eframe:

```sh
# Arch
sudo pacman -S --needed rust wayland libxkbcommon mesa pkgconf

cargo test --workspace
cargo build --release -p omalight
```

`packaging/install.sh` installs the three binaries, the desktop file, the icon, and the agent skill into `~/.local` (or `PREFIX`). `packaging/PKGBUILD` is the same install for Arch `x86_64` and `aarch64`.

```sh
omalight skill --link
```

## Keys

| Key | Action |
| --- | --- |
| `/` | Command palette |
| `?` | Key list (shift-slash) |
| `q` | Quit |
| `+` / `-` | Exposure ±0.1 EV |
| `j` / `k` | Exposure ±0.05 EV |
| `[` / `]` | Temperature ±200 K |
| `h` / `l` | Temperature ±100 K |
| `r` | Reset exposure and temperature to the import values |
| `\` | Before / after |
| `e` | Export a 2048 px JPEG |
| `0`–`5` | Rating |
| `p` / `x` / `u` | Pick / reject / unflag |
| ctrl-z / ctrl-shift-z | Undo / redo |
| esc | Close the palette or the key list |

Drop a DNG, ARW, JPEG, TIFF, or PNG on the window to open it. The palette takes a sentence ("warm these portraits slightly and export 2048 px JPEGs") and turns it into engine commands. With `[ai]` set, the sentence goes to that OpenAI-compatible server first. The rule parser covers the short cases when the server is absent.

Agents use `omalight-mcp` and `skills/omalight/SKILL.md`.

## Tests

```sh
LIGHTCRAFT_GPU=0 cargo test --workspace
```

`crates/omalight-core/tests/edit.rs` runs the engine and the client in one process over a real TCP socket. `crates/omalight-core/tests/pair.rs` covers the secret file, pairing, expiry, and revocation. `apps/omalight/tests/remote.rs` runs the engine and the client as two processes and pairs them first. Both edit tests open a generated DNG, change exposure and white balance, and export a JPEG.

Screenshots from the window live in `docs/screenshots/`. Latency from this tree's machine is in `docs/latency.md`.

## Layout

```
crates/omalight-protocol   frames, plans, theme
crates/omalight-core       engine session and client
apps/omalight              window, daemon, MCP
skills/omalight            agent skill
packaging                  desktop file, PKGBUILD, install.sh
docs/ARCHITECTURE.md       protocol, latency, Omarchy, AI
docs/QUICKSTART.md         install and first pair
```
