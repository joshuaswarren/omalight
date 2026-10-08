---
name: omalight
description: Cull, develop, mask, and export photos through the Omalight engine (local or remote LightCraft). Use when the user asks to rate, flag, adjust exposure or white balance, add a sky/subject/background mask, preview, or export from a raw or DNG library.
---

# Omalight

Omalight talks to an `omalight-engine` process. The engine holds one LightCraft session. Originals stay on the host that runs the engine. The MCP server is `omalight-mcp`. The window is `omalight`.

Install the skill links with `omalight skill --link`. That symlinks this directory into `~/.agents/skills`, `~/.claude/skills`, `~/.codex/skills`, and `~/.pi/agent/skills`.

## Reach the engine

The daemon listens on port 7421. The first start writes a secret into the config directory (mode 0600) and does not ask for one. A laptop pairs once (`omalight pair HOST`, or the code shown in the window). After that, `paired.toml` in the client's config directory is the credential. The engine secret is not a network password.

`omalight-engine devices` lists paired names. `omalight-engine revoke NAME` removes one. A hostname, including a Tailscale name, is just the address you pass to `pair` or `--connect`.

Config file, in the config directory (`~/Library/Application Support/omalight` on macOS, `~/.config/omalight` on Linux):

```toml
mode = "auto"          # auto | local | remote
host = "studio.local"
port = 7421

[ai]
base_url = "http://127.0.0.1:8080/v1"
model = "local"
api_key_env = "OMALIGHT_AI_KEY"
```

`auto` uses a paired engine it can reach, then starts a local engine. `LIGHTCRAFT_GPU=0` forces the CPU pipeline. Leave it unset on a Metal or Vulkan host.

## Tools

`omalight-mcp` speaks Content-Length JSON-RPC on stdio. Tools:

| Tool | What it does |
| --- | --- |
| `status` | Ping. Returns `ping_us` and the open photo id. |
| `open_photo` | `path` on the engine host. Import and select. |
| `develop_set` | `{control, value}` or `{values: {id: number}}`. |
| `preview` | Write a viewport JPEG to `path`. Returns `server_us`. |
| `export_jpeg` | `dest`, `long_edge`, `quality`. Path is on the engine host. |
| `execute` | Any LightCraft command id plus `params`. |
| `undo` | `edit.undo`. |
| `mask_add` | `kind`: sky, subject, background, linear, radial. |
| `plan` | Sentence in, steps out. `apply: true` runs them. |
| `heavy_task` | `kind`: mask, denoise, upscale, tag. |

`execute` is the escape hatch. Stable ids used here:

- `develop.set` with `{control, value}` or `{values}`
- `develop.beginInteraction` / `develop.endInteraction` (one undo step for a drag)
- `edit.undo` / `edit.redo`
- `photo.rate` with `{rating: 0..5}`
- `photo.flag` with `{flag: "pick"|"reject"|"none"}`
- `mask.add` with `{kind}`
- `library.import` with `{paths: [...]}`
- `library.select` with `{ids, active}`
- `omalight.task` with `{kind, ...}` (same as `heavy_task`)

Set a rating or a flag only when the user asked to cull. Do not invent file paths. Exports land on the engine host.

## Masks and the stubs

Sky, subject, background, linear, radial, luminanceRange, and colorRange go to LightCraft `mask.add`. Object and prompt masks need LightCraft's SAM feature on the engine host; this build returns `status: unsupported` and the interface string.

Denoise, upscale, and tag are the same shape and are not linked:

- `DenoiseRequest { photo, strength }` with strength in 0..=1
- `UpscaleRequest { photo, scale }` with scale 2 or 4
- `TagRequest { photo }` returning `{keywords}`

A `status` of `unsupported` means the call was understood and not run. Do not retry it with a different command id.

## Headless check

```sh
omalight --headless --connect 127.0.0.1:7421 \
  --open /path/to/photo.dng --set light.exposure=0.3 --export /tmp/out.jpg \
  --latency-json /tmp/latency.json
```

The process prints one JSON object: ping, open, draft, full, export, all in microseconds.
