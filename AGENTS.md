# AGENTS.md: AI Agent Guide for ImageGlass-Ithmb-Plugin

This file tells AI coding agents how to work with this repository. Read this first before editing any code.

## Repository Purpose

C ABI shared library plugin for [ImageGlass v10](https://imageglass.org) that enables decoding Apple `.ithmb` thumbnail files. This is a **thin FFI wrapper** around the core decoding library in `ithmb-core`.

## Architecture

```
ImageGlass (cross-platform Avalonia UI)
    ↓ ig_plugin_get_api()
ImageGlass-Ithmb-Plugin (C ABI cdylib, crate `ithmb-core-cabi`)
    ↓ FFI
ithmb-core (crates.io dependency `ithmb-core = "1.9"`, compiled statically into the cdylib)
    decoders, 53 active profiles, PhotoDB (no encoder)
```

`ithmb-core` is a normal Cargo dependency, not a runtime-loaded shared library. It is linked statically into the cdylib at build time.

## Repository Layout

```
├── src/                      # Rust cdylib source (all code lives here)
│   ├── lib.rs                # C ABI entry point, plugin lifecycle, API table construction
│   ├── codec.rs              # Codec capability, extension matching, metadata loading
│   ├── decode.rs             # Static-raster decode, pixel-buffer free, buffer registry access
│   ├── state.rs              # Plugin state, statics (OnceLocks), initialization
│   ├── strings.rs            # UTF-16 conversion helpers
│   ├── allocator.rs          # pixel_buffer_alloc / pixel_buffer_free (own allocator, not host)
│   ├── buffer_registry.rs    # Thread-safe HashMap tracking live pixel allocations
│   ├── types.rs              # #[repr(C)] ABI types mirroring ImageGlass C# SDK structs
│   └── logging.rs            # Thin wrapper around host logging callback
├── scripts/
│   ├── package.sh            # Build + package into dist/ithmb-codec-<platform>.igplugin.zip
│   ├── abi-smoke.py          # ctypes ABI smoke test through the real C entry point
│   ├── check-local.sh        # Full local CI gate (clippy + test + build + deny + gitleaks)
│   ├── check-parity.sh       # Local-vs-GitHub CI parity gate
│   └── check-parity.config   # Parity gate config (REPO_SLUG, LOCAL_CMD)
├── tests/fixtures/           # test1.ithmb (data fixture only, no test code)
├── docs/
│   ├── adr/                  # Date-named ADRs, e.g. 2026-08-19-ci-optimization.md
│   ├── CREDITS.md            # AI contribution credits
│   ├── badges/               # README badge SVGs
│   ├── logo.svg              # Project logo
│   └── decoded-example.png   # Sample decode output
├── .github/workflows/ci.yml  # CI: 3-OS build + clippy + test + deny + gitleaks + release
├── igplugin.json             # Plugin manifest (id, name, executable, kind)
├── Cargo.toml                # crate ithmb-core-cabi v1.1.3, cdylib, ithmb-core = "1.9"
├── Cargo.lock                # Locked dependency versions
├── rust-toolchain.toml       # Pins Rust 1.88.0
├── deny.toml                 # cargo-deny config (crates.io source allowlist)
├── Makefile                  # build / test / lint / check / package / clean targets
├── mise.toml                 # Tool version management (rust 1.88.0, scripts on PATH)
├── .pre-commit-config.yaml   # pre-commit hooks (whitespace, gitleaks, gitlint)
├── .commitlintrc.json        # Conventional-commit rules (type-enum, subject-case)
├── .editorconfig             # Editor style defaults
├── CHANGELOG.md              # Release notes (used as GitHub release notes)
├── CONTRIBUTING.md           # Contribution guide
├── SECURITY.md               # Security policy
├── CREDITS.md                # AI contribution credits (root copy)
└── README.md                 # Project overview + usage
```

## SE Lifecycle Artifacts

| Artifact                      | Location                                                                                              |
| ----------------------------- | ----------------------------------------------------------------------------------------------------- |
| Feature Inventory             | [docs/FEATURES.md](docs/FEATURES.md) (F-001..F-010)                                                   |
| System Specification          | [SPECIFICATION.md](SPECIFICATION.md) (567 lines)                                                      |
| Architecture                  | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)                                                          |
| Technical Debt Audit          | [TECH_DEBT_AUDIT.md](TECH_DEBT_AUDIT.md)                                                              |
| Architecture Decision Records | [docs/adr/](docs/adr/) (2 ADRs)                                                                       |
| Local CI Gate                 | [scripts/check-local.sh](scripts/check-local.sh) + [scripts/check-parity.sh](scripts/check-parity.sh) |

Feature lifecycle follows Development-Protocol docs/engineering-plugin.md §1.1: proposed → approved → applied → archived.

## Key Facts

- **Language**: Rust (cdylib), crate `ithmb-core-cabi` v1.1.3, edition 2024
- **Toolchain**: pinned to 1.88.0 by `rust-toolchain.toml` (mirrored in `mise.toml`)
- **ABI**: ImageGlass v10 native codec plugin (SDK v1.1.0)
- **Platforms**: Linux, macOS, Windows (CI builds all 3)
- **Profiles**: 53 active device profiles (prefix 1044 is disabled per iOpenPod #81)
- **Dependency**: [`ithmb-core`](https://github.com/B67687/Ithmb-Codec) `= "1.9"` from crates.io, compiled statically into the cdylib
- **Memory rule**: Plugin allocates pixel buffers via its own allocator (`libc::malloc`), not the host allocator. Whoever allocates, frees.
- **Buffer tracking**: `BufferRegistry` in `buffer_registry.rs` tracks live pixel buffers to prevent double-free and use-after-free.
- **Unit tests**: live in `src/` (`decode.rs`, `buffer_registry.rs`, `codec.rs`, `lib.rs`). `tests/` holds only the `test1.ithmb` data fixture.
- **CI**: GitHub Actions on push/PR to `main` and on `v*` tags: 3-OS build + symbol export verify, clippy, cargo test, cargo-deny, gitleaks, and release creation on tags.

## Key Commands

```bash
cargo build --release                          # Build the cdylib
cargo test                                     # Run unit tests (in src/)
cargo clippy --all-features --all-targets -- -D warnings   # CI-enforced lint
./scripts/package.sh [linux|macos|windows]     # Package into dist/ithmb-codec-<platform>.igplugin.zip
python3 scripts/abi-smoke.py tests/fixtures/test1.ithmb   # ABI smoke test (ctypes, no GUI)
./scripts/check-local.sh                       # Full local CI gate (clippy + test + build + deny + gitleaks)
./scripts/check-parity.sh                      # Assert local and GitHub CI agree on the same commit
make check                                     # Same as ./scripts/check-local.sh
```

`cargo-deny` runs pinned at 0.20.2 (musl binary): `cargo-deny --log-level warn --manifest-path ./Cargo.toml --all-features check`.

## Why `unsafe_code = "allow"`

`Cargo.toml` sets `[lints.rust] unsafe_code = "allow"` deliberately. This
crate is a C ABI cdylib whose entire surface is `extern "C"` entry points;
every exported function is `unsafe` by contract because the host (ImageGlass)
calls across the FFI boundary with raw pointers it owns. The `unsafe` blocks
inside are the bridge, and each carries a `// SAFETY:` comment stating the
invariants it relies on (null checks, struct layout, ownership). Denying the
lint would flag the ABI surface itself, the point of the crate. The other
lints stay strict per the Cargo.toml cherry-pick: clippy `all` deny, `pedantic`/`nursery`/`cargo`
warn, documented hard warns. RUSTFLAGS is empty in CI so those levels govern (see Codec saga).

## SDK v1.1.0 Contract

The plugin implements the ImageGlass SDK v1.1.0 codec contract:

- **Decode-only**: `IGCodecApi` exposes decode; the encode function pointers
  are null. Encoding is deliberately deferred: it would require an ithmb
  _encoder_ in `ithmb-core`, which does not exist yet (the upstream crate
  decodes raw formats and synthesizes samples; it has no production encoder).
- **StructSize-validated**: `IGCodecApi` and `IGCodecCapability` carry
  `StructSize` as the first field; the plugin validates it on entry before
  touching any other field, so a host/plugin SDK version mismatch fails safely
  instead of reading garbage.
- **Two-argument entry**: `ig_plugin_get_api(int32_t host_abi_version, const
IGHostApi* host_api)` (the one-argument form is the pre-1.1.0 ABI).
- **Entry-point safety**: every FFI entry runs inside `catch_unwind` so a Rust
  panic cannot unwind across the C boundary.

## load_metadata Flow

1. Read 4-byte format prefix from file
2. Try `ithmb_core::device_profiles::find_formats_by_id(prefix)` (fast path)
3. If not found, fall back to `ithmb_core::profile_db::ProfileDb::load_builtin()` + `get(prefix)` (all 53 active profiles)
4. Return correct dimensions or `NotImplemented`

## free_pixel_buffer Safety

- Always clears buffer struct fields first (prevents ImageGlass from accessing stale pointers)
- Checks `BufferRegistry` before freeing
- Uses own allocator (`allocator::pixel_buffer_free`), NOT the host allocator
- Safe during shutdown: host allocator may have been torn down, but our allocator is always available

## Relationship to Ithmb-Codec

All decoding logic lives in the upstream [`Ithmb-Codec`](https://github.com/B67687/Ithmb-Codec) repo, published to crates.io as `ithmb-core`. This plugin is the C ABI glue layer. Changes to decoding behavior belong in the upstream crate, not here.

## Async Runtime (waiver)

No async runtime (no tokio): the plugin is a synchronous C ABI cdylib — every entry point
blocks on the calling host thread and all decode paths are CPU-bound sync code. Adding an
executor would buy nothing. Revisit only if a genuinely async workload (network, timers)
appears.

## Do Not Touch / Generated Paths

- `target/`: Cargo build output, gitignored, regenerated by any build
- `dist/`: packaged `.igplugin.zip` artifacts, gitignored, produced by `scripts/package.sh`
- `.omo/`: agent workspace ephemera (ledger, plans, project context). NEVER committed to git. It is not whitelisted in `.gitignore` and must never be added.

## For Agents

- This is a thin wrapper: all decode logic is in the `ithmb-core` dependency.
- The `#[repr(C)]` ABI types in `types.rs` must match ImageGlass's C# SDK structs exactly.
- `git commit` uses `-S` (GPG sign). Author date is preserved via `GIT_COMMITTER_DATE`.
- Commit messages follow conventional commits (`.commitlintrc.json`): lowercase subject, `docs:`/`fix:`/`feat:` types.
- CI enforces the Cargo.toml clippy cherry-pick (warn-level pedantic): run `cargo clippy` before pushing; tests run via `cargo nextest run`.
- Releases are created by pushing a `v*` tag: CI builds all 3 platforms and publishes.
- Run `./scripts/check-local.sh` before pushing to match CI locally.
- Do not commit `.omo/`, `target/`, or `dist/`.
