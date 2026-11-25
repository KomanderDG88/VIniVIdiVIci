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

> **Current status: experimental. It reaches the game world and the player controls the character, at about 3 to 5 frames a second on our machine.** See [Status](#status) for what is measured and what is only reported. It is not playable, and no frame-rate gain is claimed for any change in this fork. Results reported by upstream projects belong to their builds and machines.

No game files, assets or console SDK material are included. Supply your own files outside the repository. This is an independent community project, unaffiliated with console or game publishers and hardware vendors.

## What this fork adds

The fork follows victory-fork and is merged up to its `79cfdd9` (25 November 2026). That merge brings, on by default, a frame-rate counter drawn in the corner of the picture (`VICTORY_FPS_COUNTER=0` removes it; on the Series S profile it lies under the interface plane where that is opaque) and DirectStorage reads into physical pages that wait for the title's Submit; and, off by default, `VICTORY_INDIRECT_GATHER=1`, `VICTORY_SHOT`, `VICTORY_COPY_CHANGED=1` and `VICTORY_WINDOW_UPLOAD=1`. None of these has been measured on this machine yet. The changes below are what this fork adds on top. Original authors retain their credit and history.

| Area | Change | Source |
| :--- | :--- | :--- |
| **64 lanes → 32 threads** | Paired execution for wave64 compute on 32-lane GPUs. Also includes the PR's mesh/pixel linking and related translator changes. | [#854](https://github.com/fxceless/victory-fork/pull/854) · PiloteCode |
| **Memory boundaries** | Wide shader loads and stores crossing a 64 KiB page use the mapping of each page. | [#846](https://github.com/fxceless/victory-fork/pull/846) · gomeaw |
| **Pixel helper lanes** | A helper lane reads its own value in `v_readfirstlane`, allowing the tested waterfall loop to serve it on the first pass. | [0601955](https://github.com/Marvisak/VIctory/commit/0601955361c77fd8e306631784ed084a31e5ad8f) · Vax |
| **Texture refresh** | Remap-notification overflow triggers one full revisit of placed textures. The port retains the existing memory bound and protects the batch and marker with the same lock. | [74315e7](https://github.com/PastiiJay/klbf-lab/commit/74315e765a6a7b24a926e55f9b2a192d6b8ec05a) · PastiiJay / KLBF Lab |
| **Upscaler programs and more instructions** | The FidelityFX upscaler's FSPX images are read as native programs, and several instructions and descriptor forms the translator refused are now translated. Carried onto the paired form, with a test of our own for the reads that follow a whole-wave section. Of the title's 29 upscaler programs, 28 translate, validate and create pipelines on the RTX 4060 Ti; none did before. The picture with it has not been compared with the picture without it. The Series S profile needs it to draw. | [#258](https://github.com/fxceless/victory-fork/pull/258) · Ortomanic |
| **Series S: the second plane** | On the Series S profile the title presents two planes, its picture and its interface, and only the first was shown: the profile selector, the loading pictures and the opening video were black. The second plane is now drawn over the first, and the title's upscale dispatch becomes a plain stretch. The size of a plane's description is not from Microsoft's documentation; several people report it working with the same title. | [#188](https://github.com/fxceless/victory-fork/pull/188) · sagouindupm, Filip722 |
| **Textures on an 8 GB card** | `VICTORY_TEXTURE_DROP_MIPS=N` leaves a texture's N largest levels out of its copy on the card; each level left out is three quarters of the bytes. Off by default. Its author measured it on an RTX 4060 Ti with 8 GB; our own measurement is pending. | [#610](https://github.com/fxceless/victory-fork/pull/610) · ofc0urse, Filip722 |
| **Copies between title memory and the card** | `VICTORY_CENSUS=1` logs, for each copy of title memory on the card, what is sent up, compared and fetched back. `VICTORY_SKIP_WC_BACK=1` is an experiment: what shaders wrote to memory the title reserved write-combining is left on the card and not fetched back; its authors warn of artifacts, and mark `VICTORY_SKIP_WC_COMPARE=1` unsafe. All off by default; our own measurement is pending. | [#868](https://github.com/fxceless/victory-fork/pull/868) · Chipsrain, iGoldenNinja |
| **One view per section** | `VICTORY_XMEM_PERSISTENT_VIEW=1` keeps one view of a physical-page section for DirectStorage writes instead of a map, a commit and an unmap per write. Off by default; not measured here yet. | [#231](https://github.com/fxceless/victory-fork/pull/231) · HailLucifer666 |
| **Unattended runs** | In diagnostics builds a key scripted with `VICTORY_KEYS` is held until the title's reads have seen it down (once by default, `VICTORY_KEYS_READS`) instead of for a fixed 0.2 s, which at one to three frames a second fell between two reads. Presses can repeat (`150:Enter*3/20`) or be held for a set time to walk (`430:W~5`, five seconds, also with a real pad connected), and each leaves a log line saying whether and how often the title read it. | this fork |
| **A stand-in for indirect light** | Without ray tracing the title's indirect light is next to nothing and whatever the sun does not reach is black. `VICTORY_AMBIENT=<level>` replaces what the program that resolves that light wrote with a constant: tinted (`VICTORY_AMBIENT_TINT`), shaded in creases and under objects from the scene's depth (`VICTORY_AMBIENT_SHADE`), with reflections held to it (`VICTORY_AMBIENT_REFLECT`), and tuned in a running game from a file in diagnostics builds (`VICTORY_AMBIENT_FILE`). Off by default here, and then it costs nothing; its author runs it at 0.6. Not yet tried on this machine. | Belobug's [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) (beloooo); first version by way of [#917](https://github.com/fxceless/victory-fork/pull/917) · UwUCammie |
| **Title memory in place** | `VICTORY_STRETCH=1`: a window of title memory that is one run of one section's pages is used by the card through a view of our own on that section, with no copy, no refresh and no copy-back; whatever cannot be opened that way is copied as before. It works without `VICTORY_INPLACE`. Off by default. Its author measured no clear gain on a 16 GB NVIDIA card. Measured here in one room: with the usual 4 MiB windows three of them were placed and the frame did not change; with 256 KiB windows (`VICTORY_WINDOW_KB=256`) about 800 were in place, about 280 MB of 2.3 GB, none refused by the driver, and the median frame was 312 ms against 296 to 297 ms without either switch. No gain on this machine. | [#1127](https://github.com/fxceless/victory-fork/pull/1127) · FrancescoGelosoEterea, after Belobug's `VICTORY_STRETCH` |
| **Licence** | The emulation of the console's AMD-only processor instructions that victory-fork had ported from XCL, which is GPL-2.0-only, is removed: this program is GPL-3.0-or-later and cannot carry it. VIctory's own handler does that job. Checked on an Intel processor, where the title depends on it to start. | [#1057](https://github.com/fxceless/victory-fork/pull/1057) · nullexxx |
| **Pipeline workers** | `VICTORY_PIPELINE_WORKERS=<n>` and `VICTORY_PIPELINE_NORMAL=1` set the number and the priority of the threads that make host pipelines. Off by default. | [#955](https://github.com/fxceless/victory-fork/pull/955) · LukaManstein |
| **Title watchdog** | `VICTORY_NO_WATCHDOG=1` steps over the title's own hang watchdog on one known build, checking the build and the bytes it replaces first. Off by default, and not used in the measurement below. | [#959](https://github.com/fxceless/victory-fork/pull/959) · ruvenesh |
| **Test stability** | Three intermittent test failures removed: watch tests, the files test and the frame-wait test. | [#963](https://github.com/fxceless/victory-fork/pull/963), [#952](https://github.com/fxceless/victory-fork/pull/952) · MamaLeVrai; [#978](https://github.com/fxceless/victory-fork/pull/978) · avt0m4t48 |

These are correctness changes with regression tests. They are not a claim of higher frame rates or a complete rendering fix.

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

XInput controllers are supported. With no controller connected, the focused window uses the keyboard as a pad: **WASD / IJKL** for the sticks, **Enter** for A, **Backspace** for B, and **Esc / Tab** for Menu / View. The full key map and advanced switches are in the [inherited runtime guide](README.upstream.md#getting-started).

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
| Frame rate in the world | About 3 to 5 fps by scene | Series X, yard, about 800 draws a frame: 5.0 fps measured, earlier the same day and from the hard disk. Series S, opening cinematic, about 3,000 draws: 2.9 to 3.6 fps measured (usual frames of 279 to 343 ms). Under player control: 3.2 fps measured in the minute after the report (median frame 313 ms), with an unrelated job using the processor at the time. In the interior scene with the movement prompt, nearly still, with `VICTORY_AMBIENT=0.6` and `VICTORY_TEXTURE_DROP_MIPS=1`: 3.4 fps measured over 15 s (52 frames in three batches, batch medians 287 to 297 ms), one session. In that same room, about 4,500 draws a frame, with two switches that are off by default: `VICTORY_COPY_PLAIN=alt`, turns of 20 s in one session, median frame 284 ms on against 297 ms off (3.45 against 3.28 fps; shorter on in four pairs of four); `VICTORY_SKIP_WC_BACK=1`, three sessions in the order off, on, off, median frame 297, 261 and 296 ms (3.3, 3.7 and 3.4 fps). What `VICTORY_SKIP_WC_BACK` does to the picture has not been compared yet. RTX 4060 Ti. |
| Stability | 15 minutes in one session without a crash | Series S, 924 s to the session's time limit, with `VICTORY_TEXTURE_ROOM_PERCENT=95`. A 30-minute session also reached its limit, but its shutdown was not recorded cleanly and is not counted. The same evening seven unattended Series S sessions of ten minutes with `VICTORY_TEXTURE_DROP_MIPS=1` all ran to their limit without a crash. Series X stops earlier on this machine: physical memory runs out. |
| Rendering | With `VICTORY_AMBIENT=3` (off by default) an interior scene is lit in light and in shade, with visible faults; without it the 3D world is mostly dark. Loading pictures, subtitles and prompts show | Series S: the selector and loading pictures show since #188 (measured here: two planes on Series S, one on Series X), and the session goes on into the game. The 3D picture is nearly black with detail missing, and textures look wrong; subtitles and prompts on the interface plane are legible. With `VICTORY_AMBIENT=0.6`, in one session, the walls and a bed of an interior scene could be seen, while an earlier shot of that session was still very dark. In that room, one framing at three levels set in the running game: at 3 the walls, the bed, the hair and the character are clearly brighter than at 0.6, at 12 walls and character wash out. So the dark picture answers to that light, and the level that suits a scene is not one number. What it does not mend: artifacts in hair, and the side of a face that stayed black at 0.6 with the walls lit. Series X: shadows black. |

How we measure: the frame times the layer logs about every five seconds, in the world only, given by scene. Figures come from diagnostics builds, which cost frame time. One machine, a handful of sessions.

## First in-game measurement

Recorded on 25 November 2026 at commit `abf1575`, on **Windows 11 / RTX 4060 Ti / driver 617.42 / Core i7-8700K (6 cores, 12 threads) / 32 GB of RAM**, with the title's files on a 5,400 rpm hard disk. Diagnostics build, Series X profile, `VICTORY_HEAP_SYNC_TABLES=1`, frame log and card meter on.

| Run | 60-second window | Frames | Mean rate | Frame time p5 / p50 / p95 |
| :--- | :--- | ---: | ---: | :--- |
| A | First window in the world | 222 | 3.6 fps | 161 / 262 / 429 ms |
| A | About three minutes later | 188 | 3.0 fps | 115 / 303 / 672 ms |
| B, with `VICTORY_INDIRECT_WINDOW=1` added | Last minute of the run | 306 | 5.0 fps | 161 / 195 / 242 ms |

- **Reaching the world:** the first frame with 500 draws or more came about 10 minutes after the process started. The hard disk bounded the load: about 100 reads a second at 2 to 4 MB/s in the samples taken, never idle.
- **Not a controlled comparison:** the camera was not fixed. Run A's views ranged from about 530 to 2,650 draws a frame; run B stayed near 800. At about 800 draws a usual frame took 194 ms in A and 201 ms in B, so **no gain is attributed to `VICTORY_INDIRECT_WINDOW`**.
- **Where a usual frame of about 200 ms went:** the rendering thread spent about 190 ms inside submit calls, some 90 to 100 ms preparing and 60 to 100 ms waiting for the card. Copying back what shaders wrote took 86 to 141 ms for 15 to 32 MB on its own thread. The card stood idle for 93 to 134 ms of the frame, and its own work was 73 to 99 ms. On this machine the processor-side work in series bounds the frame, not the card.
- **Limits:** diagnostics counters and occasional screenshots were on, and they cost frame time. Typically 80 to 130 draws a frame were dropped with no pipeline pending, at times more. Shadows were black in the views checked. No input was tested. Both runs were stopped by the test harness's own memory guard (under 2 GiB of physical memory free), not by a crash, after about 7 and 3 minutes in the world.

### The same evening, from an SSD

The title's files were copied to an NVMe SSD on the same machine. These runs used a test build, `abf1575` plus fxceless/victory-fork#258; #258 was merged into `main` afterwards. Diagnostics build, `VICTORY_HEAP_SYNC_TABLES=1`.

| Profile | How it went | Memory |
| :--- | :--- | :--- |
| Series X | First 3D scene of the introduction after about 4.5 minutes. Stopped at 295 s by the harness's memory guard, not by a crash. | 20.1 GiB private in the process; 201 MB of physical memory available; 43.8 GiB of commit still free |
| Series S | 800 s without a crash, closed on request. The introduction's scenes advanced. | 16.7 GiB private at most; 516 MB of physical memory available at the lowest |

- **Physical memory is the limit on this 32 GB machine**, with other programs open. The page file does not help: commit was never short.
- **Picture, Series S with #258:** dark, with incomplete geometry and faces and strong temporal artifacts. There is no matching shot without #258 to compare with, so nothing is concluded about #258 here.
- **No frame rate is reported for these runs.** An unrelated job was using about ten of the machine's twelve threads at the time, the pipeline cache was cold (about 418 ms of a 580 ms frame spent waiting for pipelines) and texture refreshes waited 89 to 342 ms a frame for the lock that mapping changes take. The log's 1.4 frames a second for the last minute measures those, not the layer.
- **Player control is still not confirmed.**

## What's next

The 3D picture on Series S is poor for what look like three separate reasons, to be told apart one setting at a time on the same build:

- **Dark:** the builder of ray-tracing structures is off by default (`VICTORY_BVH_BUILDER=1` turns it on), and the title's indirect light comes from rays: whatever the sun does not reach is black. Turned on here, the session ended in an access violation in the title's code after 230 s, so it stays off. A constant in place of that light is in as a switch (`VICTORY_AMBIENT`, from Belobug's fork). Measured in one room at 0.6, 3 and 12: the picture brightens with the level and burns out at 12, so in that room the darkness answers to that light (one room, one framing: not shown for every dark shot); a level per scene (or one that follows the title's exposure) is the open part, and a face's dark side and hair artifacts are not the light's to mend.
- **Textures:** an 8 GB card has almost no room left for the title's textures with the default reserves. With a smaller reserve for render targets 3.4 GB of textures were on the card where 2.1 GB had been, in sessions that are not otherwise comparable. With each texture's largest level left out (`VICTORY_TEXTURE_DROP_MIPS=1`, #610), which is what others with 8 GB cards report working, seven sessions of ten minutes reached the 3D scenes and ran to their time limit without a crash, and one reading showed 4.3 of 7.2 GB of the card in use; there is no paired comparison of the picture yet.
- **Edges and shimmer:** the plain stretch does not accumulate or sharpen. fxceless/victory-fork#798 makes the title use its own upscaler instead.

Then the frame rate. The largest part of a frame is the copying of title memory through the kernel, 81 to 118 ms of about 300: the copy in place and the pending synchronisation and copy-back changes (fxceless/victory-fork#665, #467, #812, #916, #857, #853) are measured one at a time, at comparable draw counts. In the room above, without those switches, the thread that submits waits about 80 ms of each 297 ms frame for the card while the thread beside it fetches card copies back, and the card has nothing of ours queued for about 150 ms. Three frames taken apart with `VICTORY_SHOT` say the same of the card's side: what the title draws took 34 ms of the card's clock in a frame of 1,700 commands and 93 ms in one of 8,400, and the layer's own passes that bring its copies of title memory up to date and compare them took several times that (frames of a capture, which weigh more than usual ones). Keeping the copies is the wall, not the drawing; of the copy-free paths that are in and off, `VICTORY_STRETCH=1` (fxceless/victory-fork#1127) covered too little of that memory here to gain anything, and `VICTORY_INPLACE=1` covers less still (12 MB of windows are in private memory at start). Measured here with `VICTORY_CENSUS=1` (#868), in windows of two to six frames of the 3D scenes: 28 to 63 MB a frame are fetched back from the card, and 93 to 95 % of them are of memory the title reserved write-combining. `VICTORY_SKIP_WC_BACK=1`, which leaves those on the card, is measured next, watching for artifacts. On this Intel machine the console-only instructions cost 8,000 to 12,000 exceptions a second in the opening cinematic, which does not bound the frame.

## Contribute

Small, attributable changes with a reproducible test are welcome. For a bug report, include the commit, hardware, driver, relevant settings, steps and expected versus actual behavior. Review logs before sharing; omit personal paths and private data.

Use **synthetic fixtures** for tests. Game executables, assets, extracted shaders and derived dumps do not belong in issues or pull requests. See the [asset policy](docs/asset-policy.md).

## Built on other people's work

**Marvisak and the VIctory contributors** built the foundation. **fxceless and the victory-fork contributors** developed the runtime this fork follows. Thanks to **PiloteCode, gomeaw, Vax and PastiiJay**, and to the contributors credited throughout the preserved history.

The [inherited README](README.upstream.md) retains earlier attribution and technical notes. Its historical performance numbers do not describe this fork. The header and VI monogram are original vector artwork created for VIniVIdiVIci.

**GPL-3.0-or-later** · [License](LICENSE) · [Source](https://github.com/KomanderDG88/VIniVIdiVIci) · [Issues](https://github.com/KomanderDG88/VIniVIdiVIci/issues)
