# SPECIFICATION.md: The Plan IS the Spec

> This spec describes the AS-BUILT Imageglass-Ithmb-Plugin: a C ABI cdylib that lets ImageGlass v10 decode Apple .ithmb thumbnail files. It was written after implementation, so every section reflects the real code, not an intention. Where the protocol template asks for a plan, this document records the decision that was actually made and shipped.
> Three layers: MACRO (system), MESO (component), MICRO (implementation). An AI executor reads this and knows exactly what the project is, how it is built, and why.

---

## How to Read This Spec

**Design influences:** Volere (Robertson & Robertson 2006): requirements shell; IEEE 830 / ISO 29148: SRS structure; Shape Up (Singer 2019): pitch format; Jackson Problem Frames (2001): domain analysis.

| Layer     | Level          | Scope                                        | Changing this requires                |
| --------- | -------------- | -------------------------------------------- | ------------------------------------- |
| **MACRO** | System         | Decisions constraining the entire project    | A learning shift (RULES.md section 5) |
| **MESO**  | Component      | Contracts between components                 | Interface renegotiation               |
| **MICRO** | Implementation | Bounds within which the executor has freedom | None: executor decides within bounds  |

**Format per section:** MACRO = system-level decision (heavy rationale); MESO = component contracts; MICRO = implementation bounds.

**Priority tiers:** Tier 1 (sections 0-7) required for ANY project; Tier 2 (8-11) production; Tier 3 (12-14) open-source/long-lived.

**How to write:** MACRO filled for every section used; MESO/MICRO optional but recommended. This project is already implemented and shipped, so the spec is a record of what exists, and future changes go through RULES.md learning shift rules.

---

## 0. Constitution (Immutable Project Rules)

The constitution constrains ALL executor actions. If an action would violate these rules, the executor MUST refuse.

### MACRO: System Principles

```
Imageglass-Ithmb-Plugin Constitution:
1. Correctness: wrong output at any speed is useless
2. No magic: explicit > implicit. Every dependency and config declared.
3. Inward dependencies: the plugin knows nothing about ImageGlass internals beyond the ABI it is handed.
4. Test what matters: one behavior per test, edge cases before happy path.
5. Fail with context: every error includes the values that caused it, not just a message.
6. Tool-first: never hand-roll what a deterministic tool handles (cargo fmt, clippy, cargo-deny, gitleaks).
7. No new runtime dependency without a Y-Statement decision recorded in section 2.
8. FFI safety: no panic may unwind across the C boundary; every unsafe block carries a SAFETY comment.
9. Whoever allocates, frees: the plugin owns its pixel buffers and frees them itself.
```

**MESO/MICRO:** Inward dependencies means the plugin never imports ImageGlass code; it only consumes the `IGHostApi` table the host passes in. FFI safety means every `extern "C"` entry point runs its body inside `catch_unwind` (see `lib.rs`, `codec.rs`, `decode.rs`), and every `unsafe` block carries a `// SAFETY:` comment stating the invariant it relies on. Whoever allocates, frees means pixel buffers are allocated with the plugin's own `libc::malloc` wrapper (`allocator.rs`) and freed in `codec_free_pixel_buffer`; the host allocator is never used for pixel data because it may be torn down during shutdown.

---

## 1. Overview & Derived Ambition

### MACRO: System Vision

```
Project name: Imageglass-Ithmb-Plugin (crate ithmb-core-cabi)
One-line: C ABI plugin for ImageGlass v10 that decodes Apple .ithmb thumbnail files using ithmb-core
Core ambition: ImageGlass v10 users can open any .ithmb thumbnail file natively, decoded correctly, without a crash, on Windows, macOS, and Linux.
Why now: ImageGlass v10 ships a native plugin SDK (v1.1.0), ithmb-core 1.9 is published on crates.io with 53 active profiles, and ImageGlass has no built-in .ithmb support.

Success criteria:
- WHEN a user opens a .ithmb file in ImageGlass v10 THEN the thumbnail renders with correct dimensions and colors
- WHEN the plugin is loaded by a host with a mismatched ABI version THEN the entry point returns null instead of crashing
- WHEN a hostile or truncated .ithmb file is decoded THEN the plugin returns an error status instead of panicking
- WHEN a release tag is pushed THEN CI builds, verifies, and packages the cdylib for all three platforms

Stakeholders: ImageGlass v10 users with Apple thumbnail databases, the ImageGlass plugin ecosystem, the single maintainer, the upstream Ithmb-Codec project.

OUT OF SCOPE (V1):
- Encoding .ithmb files: deferred until ithmb-core ships a production encoder
- Animation decoding: .ithmb is a static thumbnail format; animation pointers are null
- Color profile support: capability flag supports_color_profiles = 0
- A GUI or standalone viewer: the plugin only serves the ImageGlass host
- Non-ImageGlass hosts: the C ABI is ImageGlass-specific (SDK v1.1.0)
- A CLI tool: decoding is exercised through scripts/abi-smoke.py for testing only
```

**MESO/MICRO:** The cdylib is in scope; the upstream Ithmb-Codec repo is out of scope (decode logic lives there). The plugin is decode-only by design: `IGCodecApi` carries null encode pointers. No cloud dependencies, no telemetry, fully offline.

---

## 2. Architecture & Design Decisions

### MACRO: System Architecture

Each architecture-level decision uses the Y-Statement format:

> In the context of the situation, facing the concern, we decided for the option to achieve the goal, accepting the downside.

```
### Decision 1: Static ithmb-core crate dependency
In the context of needing .ithmb decoding logic without reimplementing it,
facing the choice between linking ithmb-core statically or loading it as a shared library at runtime,
we decided for a static Cargo dependency (`ithmb-core = "1.9"` from crates.io, compiled into the cdylib)
to achieve a single self-contained binary with no version-skew risk between plugin and codec,
accepting a larger binary and a rebuild-and-repackage cycle whenever ithmb-core releases.

Alternatives considered:
- Shared-library loading (dlopen/LoadLibrary): rejected because it adds a second artifact to ship, a version-matching problem, and a runtime failure mode that static linking eliminates.

### Decision 2: C-ABI boundary with host ABI struct-size validation
In the context of a plugin that crosses the FFI boundary into a host application we do not control,
facing the risk that a host/plugin SDK version mismatch reads struct fields out of bounds,
we decided for StructSize-first ABI tables validated on entry (added in commit 86e6745)
to achieve safe failure on version mismatch instead of reading garbage,
accepting a small validation cost on every entry point and a hard dependency on the host setting struct_size correctly.

Alternatives considered:
- Trusting the host's layout blindly: rejected because a mismatched SDK version would silently misalign every offset-based read.
- Version-only negotiation: rejected because the major-version check alone cannot catch a same-major struct layout change.

### Decision 3: 53 active profiles with prefix 1044 disabled
In the context of the built-in profile database shipped by ithmb-core,
facing a known-bad profile that produced wrong output for a real device,
we decided to disable prefix 1044 (per iOpenPod #81) leaving 53 active profiles
to achieve correct decode output for every supported device,
accepting that files from the affected device model are not decoded until the profile is fixed upstream.

Alternatives considered:
- Keeping 54 profiles: rejected because the 1044 profile produced incorrect dimensions for its device.

### Decision 4: Plugin-owned allocator for pixel buffers
In the context of allocating pixel buffers that the host will read and later ask us to free,
facing the host allocator being unavailable or torn down during shutdown,
we decided for a thin libc::malloc/free wrapper in allocator.rs
to achieve a self-contained allocation lifecycle that never depends on host state,
accepting that the plugin must track every allocation itself to avoid leaks.

Alternatives considered:
- Using the host allocator (IGHostCoreApi::alloc/free): rejected because calling it during shutdown crashes when the host has partially torn down.

### Decision 5: BufferRegistry for buffer lifecycle safety
In the context of handing raw pointers to the host and getting them back for freeing,
facing double-free and use-after-free bugs that would corrupt the host process,
we decided for a thread-safe BufferRegistry (buffer_registry.rs) tracking every live allocation
to achieve validated frees and early detection of dangling pointers,
accepting a mutex-protected map lookup on every free.

Alternatives considered:
- Trusting the host to free exactly once: rejected because a double-free in a host process is a crash we cannot debug from inside the plugin.

### Decision 6: catch_unwind on every FFI entry point
In the context of Rust panics being undefined behavior when they unwind across an extern "C" boundary,
facing the host process aborting on any unexpected panic,
we decided to wrap every entry point body in std::panic::catch_unwind
to achieve a guaranteed error status instead of a host crash,
accepting a small per-call overhead and the need to map panics to IGStatus::Internal.

Alternatives considered:
- Letting panics propagate: rejected because unwinding through C is UB and aborts the host.

### Decision 7: Decode-only plugin (encode pointers null)
In the context of the ImageGlass SDK v1.1.0 codec contract,
facing ithmb-core having no production encoder,
we decided to expose decode entry points and leave all encode pointers null
to achieve a correct, honest capability surface,
accepting that the plugin cannot write .ithmb files.

Alternatives considered:
- Stubbing encode functions: rejected because a stub that returns success would corrupt files; a stub that errors is no better than null.

### Decision 8: Pinned prebuilt CI tools (ADR-0001)
In the context of CI jobs that re-installed toolchains and re-cloned advisory databases on every run,
facing 30-60s of wasted cold-start time per job,
we decided to download pinned prebuilt binaries (cargo-deny 0.20.2 musl, gitleaks 8.24.3) directly
to achieve byte-identical checks that run faster,
accepting a hardcoded version URL that must be bumped deliberately.

Alternatives considered:
- The cargo-deny Docker action: rejected because it reinstalls the toolchain and advisory DB into a fresh container every run.
```

**MESO/MICRO:** Component interface contracts: `types.rs` mirrors the ImageGlass SDK v1.1.0 `ig_plugin_abi.h` exactly with `#[repr(C)]` layout; `IGStringRef` is UTF-16 with length in code units; booleans cross as i32 0/1; enum values are stable and append-only. Coding constraints: `unsafe_code = "allow"` is deliberate (the whole surface is FFI), but clippy `all` + `pedantic` are deny, `unused_crate_dependencies` is deny, and every unsafe block has a SAFETY comment.

### PROJECT_MODEL: Whole-Project State Machine (MANDATORY)

Every project MUST document its whole-project state machine at `docs/PROJECT_MODEL.md`. This project's state machine is documented there: states `IDEA → SPEC'D → PROTOTYPED → IMPLEMENTED → POLISHED → SHIPPED → MAINTAINED → EVOLVED`, the valid and invalid transitions, the invariants, and the blast-radius map of coupled components. The current state is **POLISHED**: the code is hardened, CI is green, and this governance documentation is the last gate before DISTRIBUTE.

**Gate check:** SPECIFICATION is incomplete without PROJECT_MODEL.md. REVIEW checks that every addition's transition is in the table.

---

## 3. File Tree & Module Responsibilities

### MACRO: Directory Structure

```
Imageglass-Ithmb-Plugin/: C ABI cdylib plugin for ImageGlass v10 (.ithmb decode)
+-- src/: all Rust source (cdylib, crate ithmb-core-cabi)
+-- scripts/: packaging, ABI smoke test, local CI gates
+-- tests/fixtures/: test1.ithmb data fixture (no test code)
+-- docs/: ADRs, credits, badges, this governance set
+-- .github/workflows/: CI (5 jobs)
+-- igplugin.json: plugin manifest (id, name, executable, kind)
+-- Cargo.toml: crate config, cdylib, ithmb-core = "1.9"
+-- deny.toml: cargo-deny policy (crates.io source allowlist)
+-- CHANGELOG.md: release notes (used as GitHub release notes)
```

**MESO: Per-Module Contracts (Parnas-style):**

```
src/types.rs: #[repr(C)] ABI types mirroring the ImageGlass SDK v1.1.0 header
  HIDES: exact struct layout, enum discriminants, offset contracts
  EXPORTS: IGPluginApi, IGCodecApi, IGHostApi, IGHostCoreApi, IGCodecCapability, IGImageInfo, IGPixelBuffer, IGStringRef, IGStatus, ig_status_from_decode_error, ig_string_ref_from_str, ig_string_ref_null
  CALLER: every other module
  Precondition: none (pure data + helpers)
  Postcondition: struct sizes match the C header (asserted by test_abi_struct_sizes)
  Invariant: IGCodecApi is 112 bytes, IGCodecCapability is 104 bytes on 64-bit

src/strings.rs: UTF-16 conversion helpers
  HIDES: the UTF-16 code-unit convention of the ABI
  EXPORTS: encode_utf16, utf16_to_string
  CALLER: state.rs, codec.rs, decode.rs, logging.rs
  Precondition: IGStringRef.data is valid for length code units
  Postcondition: lossy UTF-16 to String conversion, or None for null/empty refs
  Invariant: length counts code units, not bytes

src/allocator.rs: plugin-owned pixel buffer allocation
  HIDES: the decision to use libc::malloc/free instead of the host allocator
  EXPORTS: pixel_buffer_alloc, pixel_buffer_free
  CALLER: decode.rs
  Precondition: pixel_buffer_free receives a pointer from pixel_buffer_alloc
  Postcondition: allocation is zero-initialized (malloc) or freed
  Invariant: the plugin never calls the host allocator for pixel data

src/logging.rs: host logging wrapper
  HIDES: the IGHostCoreApi::log calling convention
  EXPORTS: Logger, LogLevel
  CALLER: lib.rs, codec.rs, decode.rs
  Precondition: the IGHostCoreApi pointer outlives every Logger call
  Postcondition: a log message is delivered to the host, or silently dropped if the pointer is null
  Invariant: Logger methods never panic

src/buffer_registry.rs: thread-safe registry of live pixel buffers
  HIDES: the HashMap keyed by raw pointer, mutex poisoning recovery
  EXPORTS: BufferRegistry, BufferEntry
  CALLER: decode.rs, state.rs
  Precondition: none
  Postcondition: register/unregister/contains/len/is_empty are consistent under the mutex
  Invariant: every buffer handed to the host is registered until freed

src/state.rs: plugin-lifetime statics and ABI tables
  HIDES: OnceLock initialization order, pointer stability guarantees
  EXPORTS: PLUGIN_EXTENSIONS, PLUGIN_STATE, CAPABILITY, HOST_API, BUFFER_REGISTRY, ensure_initialized
  CALLER: lib.rs, codec.rs, decode.rs
  Precondition: ensure_initialized runs before any table is read
  Postcondition: all statics are initialized exactly once; raw pointers are stable for the process lifetime
  Invariant: initialization order is extensions → plugin state → capability

src/codec.rs: codec capability, extension matching, metadata loading
  HIDES: the device-profiles fast path and ProfileDb fallback
  EXPORTS: codec_get_capability, codec_can_handle_extension, codec_can_handle_signature, codec_load_metadata
  CALLER: state.rs (table construction), tests
  Precondition: ensure_initialized has run
  Postcondition: capability pointer, extension match (0/1), or filled IGImageInfo
  Invariant: every entry point catches panics and returns a status code

src/decode.rs: static-raster decode and buffer lifecycle
  HIDES: the 8 MiB pre-size check, checked stride arithmetic, buffer registration
  EXPORTS: codec_decode_static_raster, codec_free_pixel_buffer
  CALLER: state.rs (table construction), tests
  Precondition: buffer is non-null, frame_index is 0, path is a valid UTF-16 ref
  Postcondition: IGPixelBuffer filled with a registered BGRA allocation, or an error status
  Invariant: decoded buffers are registered before return and unregistered before free

src/lib.rs: C ABI entry point and plugin lifecycle
  HIDES: ABI version negotiation, host API struct-size validation, table wiring
  EXPORTS: ig_plugin_get_api (the only exported symbol), get_host_api
  CALLER: ImageGlass host, scripts/abi-smoke.py
  Precondition: host_abi_version major matches, host_api is non-null and large enough
  Postcondition: a pointer to the static IGPluginApi, or null on any incompatibility
  Invariant: ig_plugin_get_api is the only #[no_mangle] export
```

**MICRO:** All modules stay under the 250-LOC non-test ceiling (enforced by review; the v1.1.0 split of lib.rs into codec.rs, decode.rs, state.rs, strings.rs was done for exactly this reason). Naming: `codec_*` for codec entry points, `plugin_*` for plugin lifecycle, `ig_*` for ABI helpers.

---

## 4. Quality Gates & Verification

### MACRO: Quality Gates

Acceptance criteria in EARS notation:

> WHEN a pull request is opened or a push lands on main
> THEN CI SHALL run all five jobs (build ×3-OS, verify_clippy, verify_deny, secrets, release-on-tag)
> WHERE any job fails
> THEN the change SHALL be rejected with the failing job's output.

> WHEN the cdylib is built
> THEN the symbol `ig_plugin_get_api` SHALL be present in the export table
> WHERE the symbol is missing
> THEN the build SHALL fail (nm -D / nm -gU on Linux/macOS, PE export-table parse on Windows).

> WHEN the ABI smoke test runs
> THEN the full codec path SHALL succeed: entry → initialize → get_codec → can_handle_extension → load_metadata → decode_static_raster → free_pixel_buffer
> WHERE any step returns a non-Ok status
> THEN the smoke test SHALL exit non-zero.

> WHEN a release tag (v*) is pushed
> THEN CI SHALL build all three platforms, package dist/*.igplugin.zip, and create a GitHub Release with CHANGELOG.md as notes.

**MESO/MICRO:** Local parity is enforced by `scripts/check-local.sh` (clippy -D warnings, cargo nextest, release build + symbol verify + ABI smoke, cargo-deny, gitleaks) and `scripts/check-parity.sh` asserts local and GitHub CI agree on the same commit. Concrete commands: `cargo clippy --all-features --all-targets -- -D warnings`, `cargo nextest run --all-features --all-targets`, `cargo build --release`, `python3 scripts/abi-smoke.py tests/fixtures/test1.ithmb`, `/tmp/cargo-deny --log-level warn --manifest-path ./Cargo.toml --all-features check`, `/tmp/gitleaks git --no-banner --log-opts="--no-merges --all"`.

---

## 5. Dependencies & External Contracts

### MACRO: System Dependencies

| Package         | Version                            | Purpose                                            | Contract                                                                                                                                                     | License            |
| --------------- | ---------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------ |
| ithmb-core      | 1.9 (crates.io, locked 1.9.9)      | .ithmb decoding, profile DB, device profiles       | `decode_ithmb(&[u8], &AtomicBool) -> Result<DecodedImage, DecodeError>`; `profile_db::ProfileDb::load_builtin()`; `device_profiles::find_formats_by_id(i32)` | MIT                |
| libc            | 0.2                                | malloc/free for plugin-owned pixel buffers, c_void | `libc::malloc` / `libc::free`                                                                                                                                | MIT / Apache-2.0   |
| Rust toolchain  | 1.88.0 (rust-toolchain.toml)       | compiler, edition 2024                             | cdylib crate-type, lints                                                                                                                                     | MIT / Apache-2.0   |
| cargo-deny      | 0.20.2 (pinned musl binary)        | supply-chain audit                                 | deny.toml policy: licenses allowlist, multiple-versions deny, crates.io-only registry                                                                        | Apache-2.0         |
| gitleaks        | 8.24.3 (pinned binary)             | secrets scan of git history                        | scans all commits, no-merges                                                                                                                                 | MIT                |
| ImageGlass host | v10, SDK v1.1.0, build 10.0.3.805+ | host services: logging, allocation, cancellation   | `ig_plugin_get_api(int32_t, const IGHostApi*)` ABI contract; host validates IGCodecApi.StructSize                                                            | proprietary (host) |

**MESO/MICRO:** The C ABI contract with the ImageGlass host is the key external contract. It is a two-argument entry point (the one-argument form is the pre-1.1.0 ABI), StructSize-first tables, UTF-16 string refs, i32 booleans, and stable append-only enum values. ithmb-core is the only decode dependency and is compiled statically into the cdylib; there is no runtime-loaded shared library. Versions are pinned: Cargo.lock is committed, rust-toolchain.toml pins 1.88.0, and CI tool binaries are pinned by exact version in their download URLs. No new runtime dependency may be added without a Y-Statement in section 2.

---

## 6. UX & Interface Contract

### MACRO: User-Facing Behavior

```
### Entry Points
- ig_plugin_get_api(host_abi_version, host_api): the single C entry point; returns the IGPluginApi table (ImageGlass host, scripts/abi-smoke.py)
- IGCodecApi functions: get_capability, can_handle_extension, can_handle_signature, load_metadata, decode_static_raster, free_pixel_buffer (ImageGlass host)
- igplugin.json: plugin manifest consumed by the ImageGlass plugin manager

### User-Facing Behavior (EARS)
WHEN ImageGlass loads the plugin
THEN the system SHALL return a valid IGPluginApi table
WHERE the host ABI major version mismatches or host_api is null or undersized
THEN the system SHALL return null.

WHEN ImageGlass asks whether an extension is supported
THEN the system SHALL return 1 for .ithmb and .ipm (case-insensitive) and 0 otherwise.

WHEN ImageGlass requests metadata for an .ithmb file
THEN the system SHALL fill IGImageInfo with dimensions from the profile database
WHERE the file is missing or unreadable
THEN the system SHALL return IoError
WHERE the file exceeds 8 MiB or has an unknown prefix
THEN the system SHALL return DecodeFailed or NotImplemented.

WHEN ImageGlass requests a decode of frame 0 of an .ithmb file
THEN the system SHALL fill IGPixelBuffer with a BGRA8 allocation
WHERE the file is hostile or truncated
THEN the system SHALL return a mapped error status and never panic.
```

**MESO: Error Contract:**

| Condition                                      | Error          | Remediation                       | Log Level |
| ---------------------------------------------- | -------------- | --------------------------------- | --------- |
| Null buffer / null info / null capability slot | InvalidArg     | Caller fixes its arguments        | info      |
| Non-zero frame index                           | InvalidArg     | Only frame 0 is supported         | info      |
| File missing or unreadable                     | IoError        | Caller surfaces the path          | error     |
| File larger than 8 MiB                         | DecodeFailed   | Rejected before reading           | warn      |
| Unknown format prefix                          | NotImplemented | Caller tries another codec        | info      |
| Decode failure (JPEG/profile)                  | DecodeFailed   | Caller shows a decode error       | error     |
| Invalid format / short buffer                  | InvalidArg     | Caller rejects the file           | warn      |
| Unsupported feature                            | Unsupported    | Caller reports unsupported        | warn      |
| Cancellation requested                         | Canceled       | Caller stops the operation        | info      |
| Allocation failure / stride overflow           | OutOfMemory    | Caller frees and retries          | error     |
| Any panic caught at the boundary               | Internal       | Caller treats as internal failure | error     |

**MICRO:** All strings crossing the boundary are UTF-16 IGStringRefs with length in code units. Pixel format is always Bgra8Unorm (1). Metadata reports sRGB, no HDR, no ICC profile, frame_count 1.

---

## 7. Timeline, Milestones & Checkpoints

### MACRO: Project Appetite

```
Appetite: the project shipped over ~5 weeks (2026-07-13 to 2026-08-16) plus hardening; the current milestone is the REVIEW gate.

| Milestone | What ships | Checkpoint | Acceptance Criteria |
|-----------|------------|------------|---------------------|
| M1 | v1.0.0 initial plugin | ABI fix landed (igplugin.json executable, FFI layout rewrite) | WHEN ImageGlass loads the plugin THEN .ithmb files open natively |
| M2 | v1.1.0 SDK v1.1.0 ABI port | StructSize-first tables, plugin-allocated capability, module split | WHEN the ABI smoke test runs THEN the full codec path passes |
| M3 | v1.1.1 ithmb-core 1.9.6 | profile 1044 disabled, 53 active profiles | WHEN a known device file decodes THEN dimensions are correct |
| M4 | v1.1.3 ithmb-core 1.9.9 | zune-jpeg migration | WHEN a grayscale JPEG decodes THEN output is correct |
| M5 | CI hardening (ADR-0001) | cargo nextest in CI, Windows PE export verify, pinned deny | WHEN CI runs THEN all 5 jobs pass |
| M6 | Standards audit fixes | 40 audit failures reduced to 9 | WHEN the audit re-runs THEN failures are resolved |
| M7 | REVIEW gate + governance docs | this SPECIFICATION, EXPLAINER, RULES, PROJECT_MODEL, FEATURES | WHEN REVIEW runs THEN Document Completeness and Protocol Compliance pass |

Circuit breaker: IF the ABI smoke test fails on any platform THEN the release SHALL be blocked until the ABI contract is restored.
Contingency: IF a decode regression appears in a released version THEN revert the ithmb-core bump and re-release the previous version.
```

**MESO/MICRO:** M1-M4 shipped as milestone releases on v* tags. M5-M7 are hardening and governance, no new features. Quality level: M1 prototype-grade ABI, M2+ production (all error states tested), M5+ release-grade (CI enforced).

---

## 8. Testing Strategy (Tier 2)

### MACRO: Test Philosophy

```
Unit coverage target: 4 of 9 modules have unit tests (lib.rs, decode.rs, buffer_registry.rs, codec.rs); overall module coverage is under 50%. Honest target: raise module coverage above 50% by adding tests for state.rs, strings.rs, allocator.rs, types.rs, logging.rs.
Integration scope: the full codec path through the real C entry point (scripts/abi-smoke.py)
E2E coverage: manual verification in ImageGlass v10 (no automated GUI tests)
Framework: Rust built-in test harness, executed via cargo nextest (0.9.143); no other external test dependencies
```

**MESO: Per-Component Test Requirements:**

| Module             | Test Type          | Target                                              | Notes                                                                                             |
| ------------------ | ------------------ | --------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| lib.rs             | unit               | entry-point validation, ABI struct sizes            | 5 tests: null host, ABI major mismatch, undersized host, valid table, struct sizes (112/104/16/8) |
| decode.rs          | unit + pseudo-fuzz | decode paths, buffer lifecycle                      | 4 tests + deterministic fuzz (3000 mutations, fixed seed, decoder never panics)                   |
| buffer_registry.rs | unit               | register/unregister/contains                        | 7 tests incl. double-register, unknown unregister, null key                                       |
| codec.rs           | unit               | capability, extension matching, metadata            | 8 tests incl. case-insensitive matching, dimension parsing, profile fixture                       |
| state.rs           | none               | :                                                   | untested directly; exercised indirectly by every FFI test via ensure_initialized                  |
| strings.rs         | none               | :                                                   | untested directly; exercised via codec/decode tests                                               |
| allocator.rs       | none               | :                                                   | untested directly; exercised via decode roundtrip                                                 |
| types.rs           | none               | :                                                   | untested directly; ig_status_from_decode_error exercised via decode tests                         |
| logging.rs         | none               | :                                                   | untested; host logging is a no-op when the host pointer is null                                   |
| integration        | ABI smoke          | scripts/abi-smoke.py through the real C entry point | runs in CI on Linux and in check-local.sh                                                         |

**MICRO:** One behavior per test. Test names describe the expected outcome (`decode_rejects_null_buffer`, not `test_decode`). Edge cases are explicit (null pointers, empty refs, hostile input). The pseudo-fuzz harness is deterministic (fixed seed 0x5EED_2026_1B8E_F00D) and asserts the one invariant that matters at the FFI boundary: the decoder never panics.

---

## 9. Operational Resilience (Tier 2)

### MACRO: Resilience Strategy

```
Error tracking: host logging via Logger (info/warn/error through IGHostCoreApi::log); read failures log the OS error with the path context
Fallback behavior: load_metadata tries device_profiles::find_formats_by_id first, then falls back to ProfileDb::load_builtin
Recovery mechanism: catch_unwind on every FFI entry converts panics to IGStatus::Internal; BufferRegistry validates frees to prevent double-free
Load handling: single decode at a time; cancellation is polled via an AtomicBool passed to ithmb-core; files over 8 MiB are rejected before reading
```

**MESO/MICRO:** Errors propagate as IGStatus codes across the FFI boundary; the plugin never throws. Log messages are prefixed `ithmb-codec:` for greppability in the host log. The plugin is safe during shutdown: pixel buffers are freed with the plugin's own allocator, which is always available even when the host allocator has been torn down.

---

## 10. Build & Release Pipeline (Tier 2)

### MACRO: Release Strategy

```
Versioning scheme: Semantic Versioning (MAJOR.MINOR.PATCH), currently 1.1.3
Release cadence: milestone-based, triggered by pushing a v* tag
Changelog: manual (CHANGELOG.md, used as GitHub release notes)
```

**MESO/MICRO:** The cdylib is the single artifact; all modules release together. `scripts/package.sh [linux|macos|windows]` builds and packages `dist/ithmb-codec-<platform>.igplugin.zip` (binary + generated igplugin.json manifest). CI builds all three platforms on every push/PR and uploads artifacts; the release job creates the GitHub Release on v* tags. Version bumps touch Cargo.toml, igplugin.json, and state.rs (the plugin_version string) together.

### MESO: Distribution Surfaces (enumeration required)

_Every product surfaces to its audience through a set of distribution surfaces. Enumerate ALL of them and when each is built._

| Surface                    | Built when?                             | Purpose / audience                                               |
| -------------------------- | --------------------------------------- | ---------------------------------------------------------------- |
| .igplugin.zip package      | every release (scripts/package.sh + CI) | the primary surface: ImageGlass v10 plugin manager installs this |
| GitHub Releases            | every v* tag (CI release job)           | distribution channel for all three platform packages             |
| C ABI (cdylib)             | every build                             | language interoperability; the FFI surface any host can call     |
| Web demo (ithmb-codec.dev) | upstream Ithmb-Codec project            | browser reach, zero-install evaluation of the codec              |
| docs.rs (ithmb-core-cabi)  | crate publish                           | API reference for the crate metadata                             |
| README + docs              | maintained continuously                 | onboarding, integration instructions, FFI example                |

Distribution surfaces differ by money tier: this is a **library/plugin** tier, so the surfaces are docs + examples + the packaged plugin. Each surface's build milestone is listed in Section 7.

---

## 11. Design for Change

Intent: How does this project make goalpost shifts cheap instead of expensive?

| Rule                                              | Applied? | How                                                                                                              |
| ------------------------------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------- |
| Interface Rule (no interface before 2nd consumer) | Yes      | The C ABI is the single interface; no internal trait abstractions were invented before a second consumer existed |
| Test Rule (contract over implementation)          | Yes      | Tests assert the ABI contract (struct sizes, status codes, extension matching), not internal implementation      |
| Module Boundary (single entry point)              | Yes      | ig_plugin_get_api is the only exported symbol; every module has one responsibility                               |
| Size Rule (250/40 LOC limits)                     | Yes      | All modules under the 250-LOC non-test ceiling; lib.rs was split in v1.1.0 to comply                             |
| Cycle Rule (shippable per cycle)                  | Yes      | Milestone-based releases on v* tags; every milestone ships a working plugin                                      |
| Appetite Rule (time before scope)                 | Yes      | Decode-only V1; encode deferred until ithmb-core ships an encoder                                                |
| AI Rule (same structural checks)                  | Yes      | AI-generated code passes the same clippy/test/deny/parity gates as human code                                    |
| Rule of Three (extract on 3rd)                    | Yes      | Module extraction happened when lib.rs outgrew its bounds (v1.1.0 split)                                         |
| Dependency Rule (core ≠ infra)                    | Yes      | ithmb-core is the only decode dependency; the plugin is thin glue with no infra coupling                         |
| Clean Backlog (no perpetual)                      | Yes      | OUT OF SCOPE items are explicit; encode is blocked on an upstream dependency, not silently carried               |

---

## 12. Documentation Strategy (Tier 3)

### MACRO: Documentation Plan

```
README: project overview, quick start, ImageGlass integration, FFI example, license
API docs: doc comments on all public items (rustdoc); no separate generated site
Tutorials: README quick start covers build, package, and install
Examples: scripts/abi-smoke.py is a working FFI example; README shows the C entry point
Migration guides: CHANGELOG.md documents ABI changes (v1.0.0 → v1.1.0 SDK port)
```

**MESO/MICRO:** Every public function has a doc comment with Safety and Panics sections where relevant. AGENTS.md documents the architecture for AI agents. docs/adr/ records significant decisions (ADR-0001). CREDITS.md (root and docs/) records AI contribution. This governance set (SPECIFICATION, EXPLAINER, RULES, PROJECT_MODEL, FEATURES) is the protocol layer. Markdown for all docs; commit messages follow Conventional Commits (.commitlintrc.json).

---

## 13. Ecosystem & Community (Tier 3)

### MACRO: Governance

```
License: MIT (SPDX: MIT)
Contribution model: accepting contributions (CONTRIBUTING.md, PR-based, single maintainer)
Code of conduct: not separately published; CONTRIBUTING.md and SECURITY.md govern behavior and reporting
Plugin API: exists. The C ABI (ig_plugin_get_api) is the plugin API for ImageGlass v10
Standards compliance: Conventional Commits, cargo-deny supply-chain policy, gitleaks secrets scan, clippy pedantic, Rust 1.88.0
```

**MESO/MICRO:** The plugin consumes the ImageGlass SDK v1.1.0 contract and the ithmb-core crate API. Upstream: Ithmb-Codec (publishes ithmb-core). Downstream: ImageGlass v10. PR requirements: CI green, tests pass, CHANGELOG updated, Conventional Commits, signed commits (-S). The plugin is decode-only; encoding contributions belong upstream in ithmb-core.

---

## 14. AI Attribution & Transparency (Tier 3)

### MACRO: Policy

```
Disclosure level: Full
Rationale: the project was built with heavy AI assistance; full transparency builds trust with users and contributors
```

**MESO: Tool Inventory:**

| Tool              | Version | Permitted Uses                       | Citation Format                |
| ----------------- | ------- | ------------------------------------ | ------------------------------ |
| DeepSeek V4 Flash | current | implementation, research, discussion | docs/CREDITS.md table          |
| OpenCode          | current | harness, agent execution             | README badge + docs/CREDITS.md |
| Oh My OpenAgent   | current | harness, agent execution             | README badge + docs/CREDITS.md |

**MICRO:** README carries "Built with AI assistance" and badge links to docs/CREDITS.md. docs/CREDITS.md lists the model, harness, and role per phase. No AI attribution appears in the binary or the .igplugin.zip package. Commit messages carry no Co-authored-by or tool attribution trailers.

---

## 15. Verification Checklist (Executor Reads Before Starting)

- [x] All template placeholders across all sections are filled
- [x] No "TODO" or "TBD" remains
- [x] Constitution (section 0) has at least 3 principles (9 present)
- [x] Out-of-scope list (section 1) is non-empty (6 items)
- [x] Each architecture decision (section 2) includes a Y-Statement (8 decisions)
- [x] Each dependency (section 5) has a version constraint
- [x] Timeline (section 7) has a circuit breaker condition
- [x] Tier 1 sections 0-7 are fully filled
- [x] Tier 2 sections 8-11 are filled for production projects
- [x] Tier 3 sections 12-14 are filled for open-source projects
- [x] **FEATURES.md** exists (docs/FEATURES.md): every IN SCOPE item is an `approved` entry; no `applied` feature lacks linked tests; statuses are valid (proposed/approved/applied/archived)
- [x] **Test anchoring**: every test references a feature ID (F-###); a test proving no feature contract is flagged, not silently carried

For engineering deliverables, also verify from the Engineering Plugin:

- [x] Quality gates (plugin §1) have concrete commands (section 4)
- [ ] Fuzz targets exist in `fuzz/` directory (Tier 2+). NOT APPLICABLE: deterministic pseudo-fuzz lives in src/decode.rs (std-only, no external fuzz crate)
- [ ] Benchmark suite exists in `benches/` (performance-sensitive). NOT APPLICABLE: decode performance is bounded by ithmb-core, not this plugin
- [ ] Snapshot testing configured (plugin §3). NOT APPLICABLE: no UI or serialized output to snapshot
- [x] cargo-deny / deny.toml exists (Tier 2+)
- [x] Multi-platform CI matrix configured (plugin §3): 3-OS matrix in ci.yml
- [ ] Test-to-source ratio meets 0.5× minimum. NOT MET: 4 of 9 modules tested; tracked as a known gap in section 8
