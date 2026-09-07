<div align="center">

<img src="docs/logo.svg?v=2" width="96" height="96">

# ImageGlass Ithmb Plugin

<a href="LICENSE"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/badges/license.svg" alt="License: MIT"></a>
<a href="https://rust-lang.org"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/badges/rust.svg" alt="Rust 1.88+"></a>
<a href="https://github.com/B67687/ImageGlass-Ithmb-Plugin/actions"><img src="https://github.com/B67687/ImageGlass-Ithmb-Plugin/actions/workflows/ci.yml/badge.svg" alt="CI"></a>

<p align="center"><a href="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/decoded-example.png"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/decoded-example.png" alt="Decoded .ithmb sample (720×480 YCbCr 4:2:0)" width="480"></a><br>
720×480 YCbCr 4:2:0, decoded by ithmb-core v1.9.6.</p>

<hr style="height:1px;background:var(--color-border-muted);border:none;">

<sub>Built with AI assistance — see <a href="./docs/CREDITS.md">CREDITS.md</a></sub>
<br>
<a href="./docs/CREDITS.md"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/badges/deepseek.svg?v=2" alt="DeepSeek"></a>
<a href="./docs/CREDITS.md"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/badges/opencode.svg" alt="OpenCode"></a>
<a href="./docs/CREDITS.md"><img src="https://cdn.jsdelivr.net/gh/B67687/ImageGlass-Ithmb-Plugin@main/docs/badges/omo.svg" alt="Oh My OpenAgent"></a>

<br>

</div>
<br>

C ABI plugin for [ImageGlass](https://imageglass.org) v10 to decode `.ithmb` thumbnail files using [ithmb-core](https://crates.io/crates/ithmb-core) from the parent repo [Ithmb-Codec](https://github.com/B67687/Ithmb-Codec)

> **Prefer a browser?** The [ITHMB Codec Web tool](https://ithmb-codec.dev/ithmb-decoder/) decodes .ithmb files online — no install needed, works on any OS, 100% private.

## Quick Start

```bash
# Build the cdylib
cargo build --release

# Run all tests (unit + pseudo-fuzz)
cargo test

# Run local CI gates (clippy, test, build, deny, gitleaks, F-### anchors, fitness)
./scripts/check-local.sh

# Package as .igplugin.zip (auto-detects host platform)
./scripts/package.sh

# Or specify a target:
# ./scripts/package.sh linux
# ./scripts/package.sh macos
# ./scripts/package.sh windows
```

Output: `dist/ithmb-codec-<platform>.igplugin.zip` (binary + manifest).

## Architecture

```
┌─────────────────────────────────────┐
│         ImageGlass Host (C#)        │
│  Plugin Loader → dlopen/LoadLibrary │
└──────────────────┬──────────────────┘
                   │ C ABI (ig_plugin_get_api)
                   ▼
┌─────────────────────────────────────┐
│     ithmb-core-cabi (this plugin)   │
│                                     │
│  lib.rs       — C ABI entry point   │
│  codec.rs     — capability, exts    │
│  decode.rs    — raster decode       │
│  buffer_registry.rs — buffer track  │
│  state.rs     — OnceLock statics    │
│  types.rs     — #[repr(C)] ABI      │
│  strings.rs   — UTF-16 helpers      │
│  allocator.rs — libc malloc/free    │
│  logging.rs   — IGHost logger       │
│  file_io.rs   — file read helpers   │
│                                     │
│  ┌───────────────────────────────┐  │
│  │    ithmb-core (dependency)    │  │
│  │  ProfileDb · decode · encode  │  │
│  └───────────────────────────────┘  │
└─────────────────────────────────────┘
```

## ImageGlass Integration (v10+)

1. Build and package: `./scripts/package.sh`
2. Open ImageGlass v10 -> **Settings -> Plugins -> Add**
3. Select the `.igplugin.zip` file from `dist/`
4. ImageGlass installs and registers the codec automatically

`.ithmb` and `.ipm` files now open natively in ImageGlass.

## Scripts

| Script                    | Purpose                                | CI equivalent                                    |
| ------------------------- | -------------------------------------- | ------------------------------------------------ |
| `scripts/check-local.sh`  | Run all local CI gates                 | Mirror of CI verify_clippy + verify_deny + build |
| `scripts/package.sh`      | Package cdylib into `.igplugin.zip`    | CI build job artifact                            |
| `scripts/abi-smoke.py`    | Load cdylib + exercise full codec path | CI build job smoke step                          |
| `scripts/check-parity.sh` | Verify local vs CI output parity       | —                                                |

## FFI from Other Languages

The library exposes a single C entry point (SDK v1.1.0 two-argument form):

```c
const IGPluginApi* ig_plugin_get_api(int32_t host_abi_version, const IGHostApi* host_api);
```

The plugin implements the ImageGlass SDK v1.1.0 codec contract (decode-only;
`IGCodecApi`/`IGCodecCapability` are StructSize-validated and the encode
function pointers are null). Requires ImageGlass build **10.0.3.805+**.

See the [ImageGlass plugin SDK](https://github.com/ImageGlass/SDK) for details.

## Features

See [docs/FEATURES.md](docs/FEATURES.md) for the full feature inventory with behavior contracts and test anchoring.

## Tech Debt

See [TECH_DEBT_AUDIT.md](TECH_DEBT_AUDIT.md) for the triaged list.

## License

MIT
