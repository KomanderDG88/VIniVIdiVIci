<p align="center">
  <img src="docs/brand/banner.svg" alt="VIniVIdiVIci — Console code. PC execution." width="1200" />
</p>

<p align="center"><strong>An experimental Xbox Series compatibility layer for Windows, with a focus on NVIDIA.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-e9bc79?style=flat-square&amp;labelColor=151e2c" alt="GPL-3.0-or-later" /></a>
  <img src="https://img.shields.io/badge/platform-Windows_x64-aab7c9?style=flat-square&amp;labelColor=151e2c" alt="Windows x64" />
  <img src="https://img.shields.io/badge/test_card-RTX_4060_Ti-aab7c9?style=flat-square&amp;labelColor=151e2c" alt="Test card: RTX 4060 Ti" />
  <img src="https://img.shields.io/badge/status-experimental-ff795f?style=flat-square&amp;labelColor=151e2c" alt="Experimental" />
</p>

<p align="center">
  <a href="#what-this-fork-adds">The changes</a> · <a href="#build-and-run">Build &amp; run</a> · <a href="#status">Status</a> · <a href="#validation">Validation</a> · <a href="#first-in-game-measurement">In-game measurement</a> · <a href="https://github.com/KomanderDG88/VIniVIdiVIci/issues">Report an issue</a>
  <br /><strong>English</strong> · <a href="README.es.md">Español</a>
</p>

---

## The project

**VIniVIdiVIci** brings selected compatibility fixes together, checks their behavior on an **RTX 4060 Ti**, and keeps their origins and limitations visible. The name gives each part of *veni, vidi, vici* its own **VI**.

Built on [victory-fork](https://github.com/fxceless/victory-fork) and [VIctory](https://github.com/Marvisak/VIctory), it runs a title's x86-64 code natively on the CPU and translates console GPU programs for Direct3D 12. The current work focuses on wave64 compute, memory correctness and texture updates.

> **Current status: experimental. It reaches the game world and the player controls the character, at 6.2 to 6.7 frames a second in the first room on our machine (scripted sessions), with dips while a zone loads.** See [Status](#status) for what is measured and what is only reported. It is not playable, and no frame-rate gain is claimed for any change in this fork. Results reported by upstream projects belong to their builds and machines.

No game files, assets or console SDK material are included. Supply your own files outside the repository. This is an independent community project, unaffiliated with console or game publishers and hardware vendors.

## What this fork adds

The fork follows victory-fork and is merged up to its `e4245f3` (26 November 2026), with more from its open pull requests and other forks merged on 27 November. These are correctness changes with regression tests. They are not a claim of higher frame rates or a complete rendering fix. What each change does, what was measured of it here and what each merge brought are in [docs/fork-changes.md](docs/fork-changes.md); the table below names each change and whose work it is.

| Area | Source |
| :--- | :--- |
| 64 lanes → 32 threads | [#854](https://github.com/fxceless/victory-fork/pull/854) · PiloteCode |
| Memory boundaries | [#846](https://github.com/fxceless/victory-fork/pull/846) · gomeaw |
| Pixel helper lanes | [0601955](https://github.com/Marvisak/VIctory/commit/0601955361c77fd8e306631784ed084a31e5ad8f) · Vax |
| Texture refresh | [74315e7](https://github.com/PastiiJay/klbf-lab/commit/74315e765a6a7b24a926e55f9b2a192d6b8ec05a) · PastiiJay / KLBF Lab |
| Upscaler programs and more instructions | [#258](https://github.com/fxceless/victory-fork/pull/258) · Ortomanic |
| Series S: the second plane | [#188](https://github.com/fxceless/victory-fork/pull/188) · sagouindupm, Filip722 |
| Textures on an 8 GB card | [#610](https://github.com/fxceless/victory-fork/pull/610) · ofc0urse, Filip722 |
| Copies between title memory and the card | [#868](https://github.com/fxceless/victory-fork/pull/868) · Chipsrain, iGoldenNinja |
| One view per section | [#231](https://github.com/fxceless/victory-fork/pull/231) · HailLucifer666 |
| Unattended runs | this fork |
| A stand-in for indirect light | Belobug's [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) (beloooo); first version by way of [#917](https://github.com/fxceless/victory-fork/pull/917) · UwUCammie |
| Title memory in place | [#1127](https://github.com/fxceless/victory-fork/pull/1127) · FrancescoGelosoEterea, after Belobug's `VICTORY_STRETCH` |
| Draws lost to a refused pipeline | [Marvisak/VIctory#1152](https://github.com/Marvisak/VIctory/pull/1152) · Pablo González, ported to this tree |
| Textures past the card's own memory | edsuuu's [VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT), after [#601](https://github.com/fxceless/victory-fork/pull/601) · DiogoPCastelos |
| The pass over the descriptor heap | [#1043](https://github.com/fxceless/victory-fork/pull/1043) and [#1190](https://github.com/fxceless/victory-fork/pull/1190) · UwUCammie |
| Windows on the card counted once | [#1250](https://github.com/fxceless/victory-fork/pull/1250) · UwUCammie |
| A short texture in place of a black one | LoverCatys/ruff-lab `ac473bf2`, after ka1qux |
| Staging memory of unchanged textures let go | LoverCatys/ruff-lab `af3322dd` |
| Console threads in compute groups of several host waves | [#1261](https://github.com/fxceless/victory-fork/pull/1261) · Garu667 |
| A raw buffer view stays inside its buffer | [#1266](https://github.com/fxceless/victory-fork/pull/1266) · lichenlang89 |
| A constant buffer view of no bytes stays one | [#1260](https://github.com/fxceless/victory-fork/pull/1260), merged in the original |
| More dynamic descriptor slots when asked | trustno1-coder/VIctory-NVIDIA `a9632bc` |
| Copies and fills done on the card instead of waited for | trustno1-coder/VIctory-NVIDIA `cc03ecb`, `007fe55` |
| The skin bake's own textures uploaded before it | [#1264](https://github.com/fxceless/victory-fork/pull/1264) · UwUCammie |
| Physical pages backed by sections of 2 MiB | [#1274](https://github.com/fxceless/victory-fork/pull/1274) · CuteBooty |
| Buffer copies let go with their view heap | [#1276](https://github.com/fxceless/victory-fork/pull/1276) · UwUCammie |
| A fair order for the textures waiting to be uploaded | ZigZagBoi1/ScarlettPort `8d32f87` (runtime/victory), Ziggy |
| A cap on the room kept for card copies not made yet | after ZigZagBoi1/ScarlettPort |
| Workers of their own for the pipelines of full-screen passes | Belobug/VIctory-Belobug-AMD `18e57db`, beloooo |
| DispatchX by thread counts | [#1292](https://github.com/fxceless/victory-fork/pull/1292) · KyleS98 |
| The structure builder's pipelines made ahead | this fork |
| Title memory by address: written only by a shader that can store by address | [#916](https://github.com/fxceless/victory-fork/pull/916) · SpaghettiMaster545 |
| Render scale and the title's own upscaler | trustno1-coder/VIctory-NVIDIA `afd84de` (from ZigZagBoi1/ScarlettPort `runtime/victory`, through vitutu1/victory-perf) |
| AMD-only instructions counted | this fork |
| A leak of placed-view shapes mended | TaynanGT/Vice-Tay PR #27 (`f07e54a`, `ce91490`, `d3626d2`), from JMAN730/project-flamingo `b1e0cd1` |
| Dynamic slots given back only to the heap that took them | [#1324](https://github.com/fxceless/victory-fork/pull/1324), [#1333](https://github.com/fxceless/victory-fork/pull/1333) (from Kamiloj9/victory-dlss `2023d5c`, PastiiJay/klbf-lab `74315e7`), [#1336](https://github.com/fxceless/victory-fork/pull/1336) · cwWwWwW |
| Mesh draws launched by the card | stellarr-studio/kelvaroq `32c4ab4` (patch 0041) |
| The skinning passes for ray tracing left out | edsuuu/VI-BR-PORT `ab4571d` |
| The structure builder's cost | this fork; avatarmanz/VIctory-7900XTX `adf5706`, `30f46c9` (from beloooo) |
| Three picture switches | miaohaixiang1-netizen/cool-fox-12gb `db556df` (from trustno1-coder/VIctory-NVIDIA); [#683](https://github.com/fxceless/victory-fork/pull/683) · Danigamer05; [#907](https://github.com/fxceless/victory-fork/pull/907) · SpaghettiMaster545 |
| AMD-only instruction sites rewritten | this fork |
| Guards, refreshes and the heap pass | [#1315](https://github.com/fxceless/victory-fork/pull/1315) · KainanP; this fork; [#1138](https://github.com/fxceless/victory-fork/pull/1138) · exotiquee; [#1146](https://github.com/fxceless/victory-fork/pull/1146) · beamdotdev |
| Copies not made again, submissions merged | LooperSalty/AnyXboxX PR #358 (#340, #337) · Skyfrye |
| Three more switches off by default | Itsbatt/victory-unified `768735a`; [#1312](https://github.com/fxceless/victory-fork/pull/1312) · Exopus; avatarmanz/VIctory-7900XTX `5f86fa3` |
| Licence | [#1057](https://github.com/fxceless/victory-fork/pull/1057) · nullexxx |
| Pipeline workers | [#955](https://github.com/fxceless/victory-fork/pull/955) · LukaManstein |
| Title watchdog | [#959](https://github.com/fxceless/victory-fork/pull/959) · ruvenesh |
| Test stability | [#963](https://github.com/fxceless/victory-fork/pull/963), [#952](https://github.com/fxceless/victory-fork/pull/952) · MamaLeVrai; [#978](https://github.com/fxceless/victory-fork/pull/978) · avt0m4t48 |
| Black-face texture fixes, the original's fifth merge | fxceless/victory-fork `e4245f3`, `93db653` · fxceless |
| Exact cascade shadows on 32 lanes | [#1002](https://github.com/fxceless/victory-fork/pull/1002) · Bruno Magalhães |
| Render targets kept across list cuts | [#1371](https://github.com/fxceless/victory-fork/pull/1371) · spk-c |
| More ray-only work left out, and counted | edsuuu/VI-BR-PORT `f1e5e45`, `24f8d2e` |
| The halves of `=alt` turns swapped on request | [#1341](https://github.com/fxceless/victory-fork/pull/1341) · Carver |
| A trim of the layer's own memory | [#1286](https://github.com/fxceless/victory-fork/pull/1286) · Vladimir |
| Loose windows reused; refreshes passing over unwritten chunks | [#1362](https://github.com/fxceless/victory-fork/pull/1362) · Mat; [#1358](https://github.com/fxceless/victory-fork/pull/1358) · TG0000 |
| Heap passes chained, textures read ahead on threads | [#1290](https://github.com/fxceless/victory-fork/pull/1290) · Exopus |
| Keys from a file; null slots counted per draw | this fork |
| The heap pass, lighter | [#1414](https://github.com/fxceless/victory-fork/pull/1414) · Kyle Soderlund |
| Translator fixes from Vice-Tay and others | TaynanGT/Vice-Tay `9bb7dbff`, `8fe3ccdd`, `17e3e2c8`, `037c5315`, `b7a99ac4`; [#682](https://github.com/fxceless/victory-fork/pull/682); #2031 (Flare) |
| Lifetimes | TaynanGT/Vice-Tay `2a267adb`; [#1377](https://github.com/fxceless/victory-fork/pull/1377) |
| From VI-BR-PORT, Belobug-AMD and the original's pull requests | edsuuu/VI-BR-PORT; Belobug/VIctory-Belobug-AMD `86dc1097`, `4b82f6c2`, #118; [#1362](https://github.com/fxceless/victory-fork/pull/1362) · Mat; [#1358](https://github.com/fxceless/victory-fork/pull/1358) · TG0000; [#1290](https://github.com/fxceless/victory-fork/pull/1290) · Exopus |
| Page mappings recorded, written clean-room | this fork (clean-room, from the idea in stellarr-studio/kelvaroq 0062) |
| Diagnostics | this fork |

## Build and run

### Requirements

- Windows 10/11 x64 and an AVX2-capable CPU.
- A Direct3D 12 GPU with Shader Model 6.6; mesh shaders are needed for world rendering. Our validation card is an NVIDIA RTX 4060 Ti.
- [Rust](https://rustup.rs/) with the MSVC C++ build tools and a Windows SDK. The repository pins Rust **1.98.1**.
- Your title files and writable cache/data folders **outside this repository**. Keep sufficient Windows committed-memory headroom available.

### 1. Get the source

```powershell
git clone https://github.com/KomanderDG88/VIniVIdiVIci.git
cd VIniVIdiVIci
Copy-Item victory.example.toml victory.toml
```

Edit `victory.toml` to point to your files. The example explains the cache and save-data paths. Then check the configuration and build:

```powershell
cargo run -p victory -- config
cargo build --release -p victory
```

### 2. Start your title

```powershell
.\target\release\victory.exe run "D:\YourTitle\title.exe" --execute
```

Replace the example path with your executable. The binary and configuration variables keep the inherited **`victory` / `VICTORY_*`** names.

XInput controllers are supported, and a Sony DualSense (the PlayStation 5's pad) connected by USB or Bluetooth is read directly as the first pad, with no driver or helper program (`VICTORY_SONY_PAD=0` leaves it alone; no rumble yet; its report parsing is tested, the pad in hand not yet). With no controller connected, the focused window uses the keyboard as a pad: **WASD / IJKL** for the sticks, **Enter** for A, **Backspace** for B, and **Esc / Tab** for Menu / View. The full key map and advanced switches are in the [inherited runtime guide](README.upstream.md#getting-started).

## Validation

Recorded on **Windows / RTX 4060 Ti / driver 617.42**. These are synthetic checks, not a game benchmark.

| Recorded checkpoint | Result | Scope |
| :--- | :--- | :--- |
| Base integration, `1dc8914` | **247 passed**, 2 ignored | `victory-shader` library suite |
| Base integration, `1dc8914` | **170 passed**, 5 ignored | `victory-gpu` library suite |
| Remap port, `158676e` | **19 passed** | Descriptor/host tests after the texture fix |
| Pixel helper test | **18 / 18** | Helper neighbours served on hardware and WARP |
| Paired LDS tests | **30 / 30 each** | Two synthetic cases, including a 128-thread group |
| Folded lane read, with #258 | **10 / 10**, 24 of 24 paired dispatches each | A wave OR read after the section's restore, lanes 0 and 63; three mutations make it fail |
| With #258 merged | **252 passed**, 2 ignored | `victory-shader` library suite |

Some device/compiler-dependent test paths can return early when prerequisites are unavailable. A suite's “passed” count alone does not certify execution of every hardware path. WARP does not cover the paired `WaveSize(32)` path here.

**GPU test suite, stabilised:** the intermittent failures of the full GPU suite (the WARP in-place-buffer test's decommit, a WARP access violation with tests running at once, and the indirect budget test's group order) are fixed by fxceless/victory-fork#878 (MamaLeVrai), #875 (eazyves) and a clamp that dispatches one group per 64 records, after #864 (xiaotu030202). Thirty runs of the whole `victory-gpu` test binary on an RTX 4060 Ti: 30 of 30 clean (171 passed, 5 ignored), against 25 of 30 before. The clamp change costs about 1.4 µs per 64 records and is not a frame-rate improvement. victory-fork's later pass that clamps scattered records once per list (`VICTORY_INDIRECT_GATHER=1`) dispatched all its groups at once and counted on the shader's old indexing; here it dispatches one group per 64 records like the other passes, with a test of 130 scattered records that fails when the single dispatch is put back. Ten runs of the binary with it: 10 of 10 clean (196 passed, 5 ignored).

The page-boundary and texture-overflow regressions were also checked by removing their respective fixes: the tests detect the resulting wrong behavior.

<details>
<summary><strong>Run the checks yourself</strong></summary>

Set `VICTORY_PC_DXC` to your Windows SDK's PC `dxc.exe` for compiler-dependent tests. GPU checks require a suitable device; inspect the output for skipped paths.

```powershell
cargo xtask install-hooks
cargo xtask policy
cargo test -p victory-shader --lib -- --nocapture --test-threads=1
cargo test -p victory-gpu --lib -- --nocapture --test-threads=1
```

For a runtime build with diagnostics:

```powershell
cargo build --profile diag -p victory --features diagnostics
```

The executable is then `target\diag\victory.exe`. See the [diagnostics reference](README.upstream.md#debugging) and [architecture](docs/architecture.md).

</details>

## Status

As of 25 November 2026, on **Windows 11 / RTX 4060 Ti / driver 617.42 / Core i7-8700K / 32 GB of RAM**, with the title's files on an NVMe SSD.

| | Status | Notes |
| :--- | :--- | :--- |
| In game | Player in control, pause menu opens | Reported by the person running it, at 18:36 in a Series S session of build `6dc5f86` (the code of `main`): the character responds to input. Not instrumented. Later, in an unattended session of `d816f3f` with a scripted Enter, the title went by itself from the start prompt through the load and the opening scenes to an interior scene that shows the game's prompt to move and to move the camera; input tried by hand there, in short taps, showed nothing that could be told apart from animation. **Confirmed the same evening with scripted input** (`VICTORY_KEYS`, build `7a262f3`, the code of `main`), nobody at the keyboard: the right stick held for four seconds turned the camera, the left stick held for four seconds turned and moved the character (views compared before and after), and the menu button opened the pause menu; the layer's log has each of those presses as read by the title. A fourth press, the stick pulled back, was not read and shows nothing either way. |
| Frame rate in the world | **6.2 to 6.7 fps** (median frame 141 to 155 ms) in the first room over the scripted sessions measured, with switches on that are off by default; the code of `main` as it is now measured 6.59 fps (median frame 144 ms), and 6.87 fps (142 ms) with `VICTORY_TABLED_ALL=1`, one session each; 7.3 fps with one more, which a checker then found at fault and is not recommended. Dips while a zone loads | First room, Series S profile, a scripted manoeuvre of 280 s (the camera turns and the character walks every 20 s; about 4,500 draws a frame), with `VICTORY_SKIP_WC_BACK=1`, `VICTORY_COPY_PLAIN=1`, `VICTORY_TEXTURE_DROP_MIPS=1` and the original's set of ten switches. Without the ten: 5.99 fps. With `VICTORY_INDIRECT_WINDOW=1` added: 7.31 fps, but the original's own checker found that switch reading a stale record, so it is not recommended. With the set the limit is the processor, not the card. Each figure is one session; the switches by name, every figure and its conditions are in [docs/measurements.md](docs/measurements.md). RTX 4060 Ti. |
| Stability | 46 minutes in one session without a crash | Series S, build `9b08008` with `VICTORY_SKIP_WC_BACK=1`, `VICTORY_COPY_PLAIN=1`, `VICTORY_AMBIENT=3` and `VICTORY_TEXTURE_DROP_MIPS=1`: 2,812 s to the session's time limit, its processes then ended by the test suite. The game's movement prompt was first seen 8 minutes in; from that first sight of the world to the end, 38 minutes 44 s, frames were logged in every five-minute window, the title read all 25 scripted presses that moved the camera and the character, and a person played part of it and walked from the first room to a corridor with other characters. The picture was looked at only at points (that first sight, the person's capture 27 minutes later), so it is not shown that the world was on screen or under control throughout. Earlier the same day: 924 s to the session's time limit, with `VICTORY_TEXTURE_ROOM_PERCENT=95`. A 30-minute session also reached its limit, but its shutdown was not recorded cleanly and is not counted. The same evening seven unattended Series S sessions of ten minutes with `VICTORY_TEXTURE_DROP_MIPS=1` all ran to their limit without a crash. Series X stops earlier on this machine: physical memory runs out. |
| Rendering | With `VICTORY_AMBIENT=3` (off by default) an interior scene is lit in light and in shade, with visible faults; without it the 3D world is mostly dark. Loading pictures, subtitles and prompts show | Series S: the selector and loading pictures show since #188 (measured here: two planes on Series S, one on Series X), and the session goes on into the game. The 3D picture is nearly black with detail missing, and textures look wrong; subtitles and prompts on the interface plane are legible. With `VICTORY_AMBIENT=0.6`, in one session, the walls and a bed of an interior scene could be seen, while an earlier shot of that session was still very dark. In that room, one framing at three levels set in the running game: at 3 the walls, the bed, the hair and the character are clearly brighter than at 0.6, at 12 walls and character wash out. So the dark picture answers to that light, and the level that suits a scene is not one number. What it does not mend: artifacts in hair, and the side of a face that stayed black at 0.6 with the walls lit; at 3 a close-up had white, glowing eyes and a corridor was too bright. The person playing the long session reported a guard's arm drawn in blocks of many colours and mouths not drawn right; the cause is not settled (see What's next). Series X: shadows black. |

How we measure: the frame times the layer logs about every five seconds, in the world only, given by scene. Figures come from diagnostics builds, which cost frame time. One machine, a handful of sessions.

## Measurements

**For an 8 GB card, as measured here** (RTX 4060 Ti 8 GB, i7-8700K, 32 GB of RAM): the Series S profile, the ten switches of Status, five more for this card and `VICTORY_TABLED_ALL=1`, all in the lines below. On the code of `main` (build `f9f53eaa`, whose `crates/` are those of `main` since the merge of 27 November), the scripted manoeuvre, one session: **6.87 fps**, median frame 142 ms, 95th percentile 169 ms; the same without `VICTORY_TABLED_ALL`, 6.59 fps and 144 ms. Keep `VICTORY_TEXTURE_DROP_MIPS=1`: without it, thousands of textures wait for room on the card and whole objects come out black. Do not set `VICTORY_DISPLAY_MODE=1920x1080` without `VICTORY_OWN_UPSCALER=1` (the world comes out black), and neither of them gives more room here.

```powershell
$env:VICTORY_SKIP_WC_BACK=1; $env:VICTORY_COPY_PLAIN=1; $env:VICTORY_TEXTURE_DROP_MIPS=1
$env:VICTORY_AMBIENT=0.6; $env:VICTORY_CARD_ROOM_FOR_TARGETS_MB=1536
$env:VICTORY_ASYNC_SUBMIT=1; $env:VICTORY_QUEUE2=1; $env:VICTORY_JOIN_PARTS=1; $env:VICTORY_BACK_OWN_VIEW=1
$env:VICTORY_INDIRECT_GATHER=1; $env:VICTORY_CARD_REHASH_RUNS=1; $env:VICTORY_CARD_HASH_LEN=1
$env:VICTORY_MERGE_GAP=64; $env:VICTORY_MERGE_RUNS=4096; $env:VICTORY_STAGING_POOL=0
$env:VICTORY_TABLED_ALL=1
.\victory.exe --console series-s <your title>
```

Scripted sessions in the first room, each figure one session. Before the merge of 27 November the code of `main` measured 6.62 fps (median frame 145 ms). Of a frame of about 148 ms, the thread that submits waits some 56 to 59 ms for the submission lock, most of it for the card to finish work whose bytes the title reads in the same frame: that wait, not the processor alone, is what bounds the frame now. In a session of 6.24 fps the thread that submits was at work 155 ms a frame, 60 to 70 ms of it its own processor time and 40 to 53 ms waiting for the card. The first in-game measurement of 25 November (3.0 to 5.0 fps), the runs from an SSD, the notes of the Status table in full and every earlier figure with its conditions are in [docs/measurements.md](docs/measurements.md).

## What's next

- **Memory first.** The process holds 16.1 to 16.8 GiB of private memory in the first room and up to 19 GiB while the world loads, on a machine of 32 GB; the peak of the load is where sessions are cut. `VICTORY_WINDOW_KB=1024` alone was measured and is not worth its cost (0.4 GiB less in the room, 5.6 % fewer frames, the peak unmoved). `VICTORY_WINDOW_TAILS` and `VICTORY_INPLACE` are next.
- **The picture.** Dark (the builder of ray-tracing structures is off by default, and `VICTORY_AMBIENT` stands in for its light), textures short of room on 8 GB, edges that shimmer; and faces and arms that turn black, reproduced on 26 November on the default profile and not mended by more light nor by baking the skin with all its sources; the original's black-face texture fixes are in `main` since the night of 26 November, and a face still came out almost all black with them in one of the two sessions that reached the cafeteria: they are not the whole answer. On the night of 26 to 27 November: without `VICTORY_TEXTURE_DROP_MIPS=1` thousands of texture slots wait for room on the 8 GB card and whole objects come out black, so that switch is needed; a 1920x1080 display does not leave more room and costs frame rate; and a face came out black while 2,611 textures were waiting in the upload queue after a change of shot, which is the next suspect. See [docs/measurements.md](docs/measurements.md).
- **The frame rate.** What bounds the frame is the layer's own submit thread, not the card: copies of title memory, the flush of refreshes, the write-watch queries and guard faults. Some twenty switches brought from other projects are in `main`, off, to be measured one at a time.
- **Loading and moving.** While the player moves or a zone loads the frame rate falls to 2 or 3 fps and worse: the texture refresh waits while the runtime remaps the title's memory.
- **Stability while loading.** A first session of the new merge, with a cold shader cache, was ended at 388 s by the title itself after a frame of 21.5 s spent waiting for the runtime's remapping; four later sessions ran to their time limit. `VICTORY_TEXTURE_NO_WAIT=1` (in `main`, off) keeps the texture refresh from waiting for that, and is to be measured next.
- **Being brought** from TaynanGT/Vice-Tay: a guard for NaN in colour exports, a comparison sampler that turned min into max (black shadows), and three translator fixes.

The full account, with its figures: [docs/whats-next.md](docs/whats-next.md).

## Contribute

Small, attributable changes with a reproducible test are welcome. For a bug report, include the commit, hardware, driver, relevant settings, steps and expected versus actual behavior. Review logs before sharing; omit personal paths and private data.

Use **synthetic fixtures** for tests. Game executables, assets, extracted shaders and derived dumps do not belong in issues or pull requests. See the [asset policy](docs/asset-policy.md).

## Built on other people's work

**Marvisak and the VIctory contributors** built the foundation. **fxceless and the victory-fork contributors** developed the runtime this fork follows. Thanks to **PiloteCode, gomeaw, Vax and PastiiJay**, and to the contributors credited throughout the preserved history.

The [inherited README](README.upstream.md) retains earlier attribution and technical notes. Its historical performance numbers do not describe this fork. The header and VI monogram are original vector artwork created for VIniVIdiVIci.

**GPL-3.0-or-later** · [License](LICENSE) · [Source](https://github.com/KomanderDG88/VIniVIdiVIci) · [Issues](https://github.com/KomanderDG88/VIniVIdiVIci/issues)
