# EXPLAINER.md: Code Explanation

> Generated at REVIEW start, before the fixed checklist. Bridges the gap between "the AI built it" and "you understand what it does and why."
> One explanation per project. Updated as the project evolves.

## For the Reader

You built a project with help from AI agents. You can't read the code directly, but you should still understand what your project does, how its parts fit together, and why certain decisions were made. This document gives you that understanding.

Think of it as a guided tour. After reading it, you should be able to describe this repo to someone else with confidence.

---

## 1. Macro Architecture

This project is a C ABI plugin that lets ImageGlass v10 open Apple .ithmb thumbnail files. It is a thin FFI wrapper: all decoding logic lives in the `ithmb-core` crate, which is compiled statically into the plugin's cdylib. The plugin exposes exactly one exported symbol, `ig_plugin_get_api`, which hands ImageGlass a function table. That table leads to a codec table with six working entry points: capability query, extension matching, signature matching, metadata loading, static-raster decode, and pixel-buffer free. The plugin owns its pixel buffers through a small allocator and tracks every live allocation in a thread-safe registry so the host can never double-free or use-after-free. Every FFI entry point runs inside `catch_unwind`, so a Rust panic can never crash the host process.

## 2. Data Flow Walk

Trigger: ImageGlass v10 opens an .ithmb file and asks the plugin to render a thumbnail.

1. ImageGlass calls `ig_plugin_get_api(host_abi_version, host_api)` in `lib.rs`. The entry point checks the ABI major version, rejects a null host pointer, and validates that the host's `IGHostApi` struct is at least as large as the plugin's own (commit 86e6745 added this guard). It stores the host pointer in `state.rs`'s `HOST_API` OnceLock, then calls `ensure_initialized`, which builds the extensions array, the plugin state (string buffers plus the `IGPluginApi` and `IGCodecApi` tables), and the codec capability. The host gets back a pointer to the static `IGPluginApi`.
2. ImageGlass calls `get_codec(0, ...)` to get the codec table, then `get_capability` in `codec.rs`, which returns the plugin-allocated `IGCodecCapability` advertising two decode extensions (.ithmb, .ipm), static-raster decode support, and no encode or animation support.
3. ImageGlass calls `can_handle_extension(".ithmb")` in `codec.rs`. The function compares the UTF-16 ref case-insensitively against the two known extensions and returns 1.
4. ImageGlass calls `load_metadata(path, ...)` in `codec.rs`. The function converts the UTF-16 path via `strings.rs`, stat-checks the file (rejecting anything over 8 MiB before reading), reads the first 4 bytes as the format prefix, and looks up dimensions. It tries `ithmb_core::device_profiles::find_formats_by_id` first, then falls back to `ithmb_core::profile_db::ProfileDb::load_builtin`. It fills the host's `IGImageInfo` with width, height, Bgra8Unorm, sRGB, and frame count 1.
5. ImageGlass calls `decode_static_raster(path, 0, buffer, ...)` in `decode.rs`. The function validates the buffer and frame index, reads the file, and calls `ithmb_core::decode_ithmb(&file_bytes, &canceled)` with a cancellation flag. On success it computes stride and buffer size with checked arithmetic, allocates the pixel buffer through `allocator.rs` (`libc::malloc`), copies the decoded BGRA bytes in, registers the pointer in the `BufferRegistry` (`buffer_registry.rs`), and fills the host's `IGPixelBuffer`.
6. ImageGlass renders the thumbnail from the buffer, then calls `free_pixel_buffer(buffer)` in `decode.rs`. The function zeroes the struct fields first (so the host never reads stale pointers), unregisters the pointer from the `BufferRegistry`, and frees it through `allocator.rs`.

End state: the thumbnail is on screen, the buffer is freed, and the registry is empty again.

## 3. Module Breakdown

Modules in dependency order (foundation first, higher-level later):

| Module             | Responsibility                                                  | Public API                                                                                                                                                                   | Key Types                                        |
| ------------------ | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| types.rs           | #[repr(C)] ABI types mirroring the ImageGlass SDK v1.1.0 header | IGPluginApi, IGCodecApi, IGHostApi, IGHostCoreApi, IGCodecCapability, IGImageInfo, IGPixelBuffer, IGStringRef, IGStatus, ig_status_from_decode_error, ig_string_ref_from_str | IGStatus, IGStringRef, IGPixelBuffer, IGCodecApi |
| strings.rs         | UTF-16 conversion helpers for the ABI                           | encode_utf16, utf16_to_string                                                                                                                                                | IGStringRef                                      |
| allocator.rs       | Plugin-owned pixel buffer allocation                            | pixel_buffer_alloc, pixel_buffer_free                                                                                                                                        | raw pointers                                     |
| logging.rs         | Thin wrapper around the host's log callback                     | Logger, LogLevel                                                                                                                                                             | Logger, LogLevel                                 |
| buffer_registry.rs | Thread-safe registry of live pixel buffers                      | BufferRegistry, BufferEntry                                                                                                                                                  | BufferRegistry, BufferEntry                      |
| state.rs           | Plugin-lifetime statics and ABI tables                          | PLUGIN_EXTENSIONS, PLUGIN_STATE, CAPABILITY, HOST_API, BUFFER_REGISTRY, ensure_initialized                                                                                   | PluginState, ExtensionsArray, HostApiPtr         |
| codec.rs           | Capability query, extension matching, metadata loading          | codec_get_capability, codec_can_handle_extension, codec_can_handle_signature, codec_load_metadata                                                                            | IGCodecCapability, IGImageInfo                   |
| decode.rs          | Static-raster decode and buffer lifecycle                       | codec_decode_static_raster, codec_free_pixel_buffer                                                                                                                          | IGPixelBuffer                                    |
| lib.rs             | C ABI entry point and plugin lifecycle                          | ig_plugin_get_api, get_host_api                                                                                                                                              | IGPluginApi                                      |

Every module has a single responsibility. The most complex module is codec.rs: it hides the two-path metadata lookup (device profiles fast path, then ProfileDb fallback) and the dimension-string parser.

## 4. Key Decisions

**Static ithmb-core dependency.** We needed .ithmb decoding without reimplementing it. We considered loading ithmb-core as a shared library at runtime, but that would add a second artifact to ship and a version-matching problem. We chose a static Cargo dependency (`ithmb-core = "1.9"` from crates.io) compiled into the cdylib. The tradeoff is a larger binary and a rebuild-and-repackage cycle whenever ithmb-core releases, but the plugin is always self-consistent with its codec.

**C-ABI struct-size validation.** The plugin crosses an FFI boundary into a host we do not control. A host built against a different SDK version could read struct fields out of bounds. We chose StructSize-first ABI tables validated on entry (commit 86e6745): the entry point checks the host's `IGHostApi.struct_size` before touching any other field, and the host validates the plugin's `IGCodecApi.struct_size` in return. The tradeoff is a small validation cost per call, but a version mismatch now fails safely instead of reading garbage.

**53 profiles, not 54.** The built-in profile database shipped 54 profiles, but prefix 1044 produced wrong output for its device. We disabled it per iOpenPod #81, leaving 53 active profiles. The tradeoff is that files from that device model are not decoded until the profile is fixed upstream, but every decoded file is now correct.

**Plugin-owned allocator.** The ImageGlass SDK rule is "whoever allocates, frees." We allocate pixel buffers with our own `libc::malloc` wrapper rather than the host allocator, because calling the host allocator during shutdown crashes when the host has partially torn down. The tradeoff is that we must track every allocation ourselves, which is exactly what the BufferRegistry does.

**Decode-only surface.** ithmb-core has no production encoder, so the plugin exposes decode entry points and leaves all encode pointers null. The tradeoff is that the plugin cannot write .ithmb files, but the capability surface is honest: no stub that could corrupt a file.

## 5. Quality Guarantees

- **Tests:** Unit tests live in src/ across four modules. lib.rs tests the entry point (null host, ABI mismatch, undersized host, valid table) and asserts the ABI struct sizes (IGCodecApi 112, IGCodecCapability 104, IGAnimationInfo 16, IGAnimationFrameInfo 8). decode.rs tests the decode path and buffer lifecycle, plus a deterministic pseudo-fuzz harness that feeds 3000 mutated inputs to the decoder and asserts it never panics. buffer_registry.rs tests register/unregister/contains including double-register and unknown-pointer errors. codec.rs tests capability, case-insensitive extension matching, dimension parsing, and metadata loading against a real profile fixture. An integration smoke test (scripts/abi-smoke.py) drives the real C entry point through the full codec path without the ImageGlass GUI.
- **Invariants:** No panic may unwind across the C boundary (every entry point is wrapped in catch_unwind). Every buffer handed to the host is registered until freed. The decoder never panics on hostile input. Struct sizes match the C header exactly.
- **Safety guarantees:** Rust gives compile-time memory safety; the unsafe surface is confined to the FFI boundary and every unsafe block carries a SAFETY comment. The plugin uses checked arithmetic for buffer sizing, rejects files over 8 MiB before reading, and validates every pointer before dereferencing.
- **Automated checks:** CI runs five jobs on every push/PR: 3-OS build with symbol-export verification (including a Windows PE export-table parse), clippy with -D warnings, cargo nextest, cargo-deny, and gitleaks. scripts/check-local.sh runs the same gates locally, and scripts/check-parity.sh asserts local and GitHub CI agree on the same commit.

**Honest limits:** only 4 of 9 modules have direct unit tests (state.rs, strings.rs, allocator.rs, types.rs, and logging.rs are exercised indirectly or not at all), so module coverage is under 50%. There is no automated GUI test against real ImageGlass; the smoke test is the closest proxy. The pseudo-fuzz harness is deterministic and std-only, not a full cargo-fuzz campaign. These are known gaps, tracked in SPECIFICATION.md section 8.

---

## Mandatory Check

After reading this explanation, can you answer these questions in your own words?

1. What does this project do, and what are its main pieces? A C ABI plugin that decodes .ithmb files for ImageGlass v10; the pieces are the entry point (lib.rs), the codec surface (codec.rs), the decode path (decode.rs), the state (state.rs), and the supporting helpers (types, strings, allocator, logging, buffer registry).
2. What happens from start to finish when you trigger the main action? ImageGlass calls ig_plugin_get_api, gets the tables, matches the extension, loads metadata, decodes frame 0 into a registered BGRA buffer, renders it, and frees the buffer.
3. Which module has the most complexity, and what does it hide? codec.rs hides the two-path metadata lookup and the dimension-string parser.
4. What was the hardest design decision, and why was it made that way? The C-ABI struct-size validation, because a host/plugin SDK mismatch would otherwise read memory out of bounds.
5. What would break first if something went wrong, and how would you know? A decode regression would surface as a wrong image or an error status in the ABI smoke test; a buffer lifecycle bug would surface as a registry mismatch or a double-free caught by the host.
