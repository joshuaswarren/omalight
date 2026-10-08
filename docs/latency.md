# Latency

Single localhost run of `omalight --headless` against `omalight-engine`. CPU pipeline (`LIGHTCRAFT_GPU=0`). Debug build; LightCraft and the other dependencies are compiled at opt-level 2. The viewport asked for was 1280×800. Draft is that viewport divided by 4 (here 267×200) at JPEG quality 55. Full is the viewport fit (here 1067×800) at JPEG quality 82. A small source is scaled up to the viewport, so the 128×96 DNG and the 640×480 DNG cost about the same at this viewport.

Times are microseconds on the machine that produced `docs/screenshots/`.

| Path | 128×96 DNG | 640×480 DNG |
| --- | ---: | ---: |
| ping round trip | 283 | 252 |
| open (import + select) | 1531 | 1629 |
| draft render (`server_us`) | 6050 | 11276 |
| full render (`server_us`) | 65520 | 75395 |
| preview round trip (draft + full) | 90915 | 106537 |
| export, long edge 256 px, server | 1628 | 7252 |
| export round trip | 1928 | 8007 |

`server_us` is the engine clock around the LightCraft render or export. The round trip is the client clock from the write until the terminal frame, including JPEG encode and the socket. Ping is answered on the reader thread, so it is not queued behind a render.

The separate-process test (`apps/omalight/tests/remote.rs`) uses the 128×96 fixture, the same viewport, and the same CPU flag. It checks that each of these fields is a positive duration. It does not lock these numbers.

A Metal or Vulkan host, and a path that is not localhost, are not in this table. The frames are the same.
