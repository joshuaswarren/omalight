# Architecture

Omalight is a window, a daemon, and an MCP server around one LightCraft session. LightCraft owns decode, develop, and render. This repo owns the channel, the Omarchy window, and the agent surface.

## Crates

| Path | Role |
| --- | --- |
| `crates/omalight-protocol` | Frames, auth checks, theme parser, edit plans. No LightCraft. |
| `crates/omalight-core` | Daemon, blocking client, JPEG, local exposure proxy, heavy-task routing. |
| `apps/omalight` | `omalight` window, `omalight-engine`, `omalight-mcp`. |

`omalight-engine` is one process, one `lightcraft_engine::Session`, one render at a time. Connections share that session under a mutex. A reader thread on each connection answers `ping` immediately and queues the rest. Preview jobs in a burst collapse to the newest generation. Other jobs keep their order and run before that preview.

## LightCraft as a git dependency

LightCraft is pulled in as a Cargo git dependency, pinned to revision `eba0b9723bfa0903a319879af854c46b9804f153`. Cargo rewrites that repo's path dependencies when the git repo is the source, so a vendored fork is not required to build. The pin is what makes a rebuild use the same pipeline. The `sam` feature is off: it pulls a model runtime this pass does not exercise. Sky, subject, and background masks work without it.

`LIGHTCRAFT_GPU=0` (or `lightcraft_gpu::set_enabled(false)`) stays on the CPU pipeline. A Mac Studio leaves the variable unset and uses Metal. A Linux GPU box leaves it unset and uses Vulkan. The hello reply names the engine `lightcraft` or `lightcraft-cpu`.

## Transport

The channel is a persistent TCP connection with `TCP_NODELAY`. Each frame is:

```
u32be length
u8    version (1)
u8    kind
u64be id
payload of (length - 10) bytes
```

The length counts version, kind, id, and payload. The cap is 32 MiB. Command payloads are JSON. A preview image is `u32be` JSON length, JSON metadata, then raw JPEG bytes. There is no base64.

Kinds: hello, hello-ok, ping, pong, command, command-result, preview, preview-image, export, export-result, error, cancel, open, open-result, pair-start, pair-offer, pair-confirm, pair-result.

The client takes `host:port`. The system resolver handles the lookup.
A MagicDNS name and any other hostname use the same resolver path.

### Network boundary

The default listener binds `0.0.0.0:7421`, which accepts connections on every IPv4 interface.
It advertises over mDNS unless `--no-mdns` is set or the bind address is loopback.
Discovery does not authenticate an engine.

The transport is plain TCP. It does not encrypt credentials, commands, photo paths or preview images.
Pairing does not add transport encryption or make an untrusted network safe.
Restrict network access to trusted peers and use an encrypted overlay or tunnel when traffic crosses other networks.

For a manually started engine that needs only local clients:

```sh
omalight-engine --bind 127.0.0.1:7421 --no-mdns
```

This changes that process's listener, not the installed service configuration.

## Auth

The engine creates a 32-byte secret the first time it starts and stores it mode 0600 in the config directory (`OMALIGHT_CONFIG_DIR`, `~/Library/Application Support/omalight` on macOS, otherwise the XDG config directory). The user is not asked for it. The secret authenticates a loopback connection. It is never accepted from any other address, and it is never sent to a client.

A device pairs once. The client sends an X25519 public key (`pair-start`). The engine replies with its public key (`pair-offer`) and does not send the code. Both sides compute a six-digit code from SHA-256 over the label `omalight-pair-v1`, the two public keys, and the shared secret. The engine prints the code and posts a desktop notification when `notify-send` or, on macOS, `osascript` is available. The user confirms on the client. The client sends the code it computed (`pair-confirm`). The engine compares in constant time and, on a match, writes a per-device credential into `devices.toml` and returns it (`pair-result`). The client stores that credential in `paired.toml` and reconnects with `hello`. A reject stores nothing. The window expires after 120 seconds. An address that starts pairing too often, or misses the code too often, is refused for the rest of that window.

On Linux, a loopback connection from the same uid (`SO_PEERCRED`) with `hello.local` set skips pairing. Other systems have no peer uid on TCP, so a local client reads the mode-0600 secret and presents it. That presentation works only on loopback.

`omalight-engine devices` lists paired names. `omalight-engine revoke NAME` removes one; the running engine reloads the file on the next connection. `omalight pair HOST` tries SSH (`omalight-engine pair-ssh NAME` on the far side, which prints a device credential and not the secret) and otherwise uses the code.

The engine advertises `_omalight._tcp` when it is not bound to loopback. The window browses that type and lists what it finds. It does not pair with the first answer on its own.

Commands before a successful `hello` are refused. `pair-start` is the only unauthenticated first frame, and it cannot open or edit a photo.

## Filesystem and session ownership

The protocol carries paths, not original photo uploads or exported image downloads.
The process that handles a path determines its filesystem and permissions:

| Operation | Filesystem | Behavior |
| --- | --- | --- |
| GUI file argument/drop, headless `--open`, MCP `open_photo.path` | Engine | Reads the original from the supplied path |
| GUI export, headless `--export`, MCP `export_jpeg.dest` | Engine | Creates missing parent directories and writes the exported image |
| MCP `preview.path` | MCP process | Writes the preview JPEG received over the connection |

Relative paths resolve from that process's working directory.
The same path text can refer to different files on the laptop and engine.
A remote GUI file drop works only when the engine can read the dropped path.

Exports use `std::fs::write`: an existing destination is truncated before the new bytes are written.
The export path is not restricted to a library directory.
Choose a new destination if an earlier export must be preserved.

Connections share the catalog, selection, develop interactions and undo state described above.
A device credential does not create a separate editing session or filesystem sandbox.
The README lists the current single-editor limit.

## Latency

A slider drag has to feel local. The window does four things:

1. On change, it multiplies the last preview's RGBA by `2^(ΔEV)` and shifts red and blue for `ΔK`. That paint is local. It is a display stand-in, not the develop pipeline.
2. Edits are held for 16 ms, then the latest exposure and temperature are sent as one preview. A burst on the server keeps only the newest generation.
3. The server renders a draft first: the viewport divided by 4, clamped to 160–640 by 120–480, JPEG quality 55, LightCraft `Quality::Draft`. Then a full pass, clamped to 2048 px, JPEG quality 82.
4. If a newer generation arrived between draft and full, the full pass is not started. The server sends `stage: cancelled`, which is a terminal frame, so the client does not wait.

`begin` and `end` map to `develop.beginInteraction` and `develop.endInteraction`, so one drag is one undo step. `commit: false` renders values without writing them (the before/after key).

`ping` is answered on the reader thread, so its round trip is not stuck behind a render. `server_us` on a preview is time inside the render, measured on the engine. `preview_rtt_us` is the client clock from the preview write until the terminal frame. Numbers from this machine are in `docs/latency.md`. They are localhost. Any other network uses the same frames.

## Window

egui 0.33 on eframe. LightCraft's own UI is egui, and eframe's winit backend is a native Wayland client. The app id is `omalight`, which matches `StartupWMClass` in the desktop file. Immediate mode keeps the develop surface to two sliders, a preview, and a palette. Rust 1.90 is LightCraft's floor; egui 0.33 is the newest release that builds on that compiler (0.34 asks for 1.92).

Theme files, first hit wins:

1. `~/.local/state/omarchy/current/theme/colors.toml`
2. `~/.config/omarchy/current/theme/colors.toml`
3. `~/.config/omarchy/theme.name` or `~/.config/omarchy/themes/<name>/colors.toml`
4. `/usr/share/omarchy/themes/<name>/colors.toml`

Canonical keys are `background`, `foreground`, `accent`, `red`, `muted`. Legacy `bg` and `fg` are accepted. The watcher polls modification times every 400 ms. With no Omarchy theme on the machine, the built-in dark palette is used.

Keys are in the README. There is no file dialog; drop a file or pass a path that the engine can read.
The window reports the engine's returned export path.

`omalight --screenshot DIR` forces a local engine, opens a generated 480×320 DNG, waits for the full preview, and writes `develop.png` and `palette.png`.

## AI and MCP

`omalight-mcp` is a Content-Length JSON-RPC process on stdio, with the same framing as LSP.
It uses the window's pairing file and can spawn a local engine.
Tools are listed in `skills/omalight/SKILL.md`.
Preview JPEGs use the caller's output path on the MCP filesystem, as described above.
They are not embedded in the JSON-RPC message.

The command palette calls `plan_request`.
With `[ai] base_url` set, it sends an OpenAI-compatible `POST {base}/chat/completions` with temperature 0.
The model must return one JSON plan.
If the request fails or no endpoint is configured, a rule parser handles sentences it recognises.
Running the plan sends engine commands.

The GUI worker defers exposure and temperature to a slider update after its plan loop.
The README explains the resulting edit/export ordering limit. Undo uses `edit.undo`.

Heavy work is supposed to run on the engine host, next to the originals. `omalight.task` dispatches:

| Kind | This pass |
| --- | --- |
| mask sky, subject, background, linear, radial, luminanceRange, colorRange | `mask.add` |
| mask object, prompt | Stub. Needs LightCraft's SAM feature on the engine. |
| denoise | Stub. `DenoiseRequest { photo, strength }`. |
| upscale | Stub. `UpscaleRequest { photo, scale }` with scale 2 or 4. |
| tag | Stub. `TagRequest { photo }` returns `{ keywords }`. |

A stub returns `status: unsupported` and the interface string. It does not pretend to have edited the photo.

## What the first pass renders

Tests open a generated Bayer DNG (uncompressed, RGGB, two colour matrices). Sony ARW uses LightCraft's ARW decoder and then the same develop path. This tree does not ship a camera raw.

The in-process test (`crates/omalight-core/tests/edit.rs`) binds `127.0.0.1:0`, checks that a bad credential is rejected, then exposure, undo, white balance, a sky mask, the denoise stub, and a JPEG export. `crates/omalight-core/tests/pair.rs` covers secret creation, approve, reject, expiry, revocation, and an mDNS advertise/browse attempt. The separate-process test (`apps/omalight/tests/remote.rs`) starts `omalight-engine`, pairs with `omalight pair`, and then runs `omalight --headless`.

## Roadmap

- Persist the LightCraft library and show a filmstrip.
- Surface the GPU backend name in the window. The hello frame already carries it.
- Enable LightCraft's SAM feature on the engine host for object and prompt masks.
- Fill `DenoiseRequest`, `UpscaleRequest`, and `TagRequest` on the engine host behind the same `omalight.task` id.
- Record the same latency table on a LAN. The frames do not change.
- Formats LightCraft still rejects stay rejected; the import error is the decoder's message.
