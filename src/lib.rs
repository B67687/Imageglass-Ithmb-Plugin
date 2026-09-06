//! C ABI entry point and API tables for the ithmb-core-cabi dynamic library.
//!
//! This crate compiles as a cdylib (`.so` / `.dylib` / `.dll`) that
//! implements the `ImageGlass` v10 native plugin ABI.  Any language that
//! can call C functions can load this library and use it to decode .ithmb files.
//!
//! ## Public C API
//!
//! The only symbol exported by this library is:
//!
//! ```c
//! const IGPluginApi* ig_plugin_get_api(int32_t host_abi_version,
//!                                      const IGHostApi* host_api);
//! ```
//!
//! Call this to obtain the plugin API table, which exposes:
//! - `get_codec` — enumerate codecs (one static-raster codec for .ithmb)
//! - `initialize` / `shutdown` — plugin lifecycle
//! - `self_test` — trivial health check
//!
//! Each codec exposes a second function table (`IGCodecApi`) with methods for
//! capability query, extension matching, metadata loading, and raster decode.
//!
//! ## Module layout
//!
//! - [`state`] — plugin-lifetime statics, string buffers, and ABI tables
//! - [`codec`] — capability / extension / metadata entry points
//! - [`decode`] — static-raster decode path and buffer lifecycle
//! - [`strings`] — UTF-16 conversion helpers

// The usize ↔ i32 casts are required by the ImageGlass ABI (all length
// fields are `i32`).  Our strings are tiny; truncation is impossible.
// Similarly, the `i32` → `usize` casts are guarded by `len >= 0` checks.
// The `u16` → `u8` casts in ASCII-comparison helpers are safe because
// our extensions are pure ASCII.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod allocator;
pub mod buffer_registry;
pub mod logging;
pub mod types;

mod codec;
mod decode;
mod file_io;
mod state;
mod strings;

use std::panic::catch_unwind;

use crate::logging::Logger;
use crate::state::{HOST_API, HostApiPtr, PLUGIN_STATE, ensure_initialized};
use crate::types::{IGCodecApi, IGHostApi, IGPluginApi, IGStatus};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The ABI version this plugin implements (v1.0.0.0).
pub(crate) const IG_PLUGIN_ABI_VERSION: i32 = 1_000_000;

/// Maximum accepted input file size (8 MiB).
///
/// Real .ithmb thumbnails are a few hundred KiB at most; anything larger is
/// either not a thumbnail or a hostile input, so it is rejected BEFORE
/// reading rather than pulled into memory.
pub(crate) const MAX_FILE_SIZE_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Plugin API implementation
// ---------------------------------------------------------------------------

/// Returns the [`IGCodecApi`] for the codec at the given index.
///
/// We expose exactly one codec (index 0).  All other indices write a null
/// pointer and return success.
/// # Safety
///
/// No preconditions beyond the ABI: a null `codec` out-pointer is rejected
/// with `InvalidArg`. Any non-null `codec` must reference a writable
/// `IGCodecApi*` slot owned by the host.
pub(crate) unsafe extern "C" fn plugin_get_codec(
    index: i32,
    codec: *mut *const IGCodecApi,
) -> IGStatus {
    let result = catch_unwind(|| -> IGStatus {
        if codec.is_null() {
            return IGStatus::InvalidArg;
        }
        if index != 0 {
            // SAFETY: `codec` was validated non-null above; points to a
            // host-owned writable `IGCodecApi*` slot.
            unsafe {
                *codec = std::ptr::null();
            }
            return IGStatus::Ok;
        }
        let Some(state) = PLUGIN_STATE.get() else {
            return IGStatus::Internal;
        };
        // SAFETY: `codec` was validated non-null above and points to a writable
        // `IGCodecApi*` slot owned by the host; the table lives in the
        // plugin-lifetime `PluginState`.
        unsafe {
            *codec = std::ptr::from_ref(&state.codec_api);
        }
        IGStatus::Ok
    });

    result.unwrap_or(IGStatus::Internal)
}

/// Plugin initialisation — the host API was already stored in the entry
/// point, so this is a no-op.
///
/// # Safety
///
/// No preconditions — takes no arguments.
pub(crate) unsafe extern "C" fn plugin_initialize() -> IGStatus {
    IGStatus::Ok
}

/// Shuts down the plugin.
///
/// # Safety
///
/// No preconditions — takes no arguments. The host API pointer read
/// inside was stored at entry and stays valid until process unload.
pub(crate) unsafe extern "C" fn plugin_shutdown() {
    let _ = catch_unwind(|| {
        if let Some(host_ptr) = HOST_API.get() {
            // SAFETY: the host pointer is still valid during shutdown.
            let host_api = unsafe { &*host_ptr.0 };
            if !host_api.core.is_null() {
                let logger = Logger::new(host_api.core);
                // SAFETY: Logger::info is safe to call; host_api verified non-null above.
                unsafe {
                    logger.info("ithmb-codec: shutdown");
                }
            }
        }
    });
}

/// Trivial self-test — always passes.
///
/// # Safety
///
/// No preconditions — takes no arguments.
pub(crate) unsafe extern "C" fn plugin_self_test() -> IGStatus {
    IGStatus::Ok
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Returns a reference to the stored host API, if available.
///
/// This is used by other modules (e.g., the logging and allocation wrappers)
/// to access host services.
#[must_use]
pub fn get_host_api() -> Option<&'static IGHostApi> {
    HOST_API.get().map(|ptr| {
        // SAFETY: the host API pointer was stored during `ig_plugin_get_api()`
        // and is valid for the entire lifetime of the plugin (guaranteed by
        // ImageGlass).
        unsafe { &*ptr.0 }
    })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// C ABI entry point — returns the [`IGPluginApi`] function table.
///
/// This is the only public symbol exported by the cdylib.  `ImageGlass` calls
/// it to obtain the plugin's function table, which it then uses to enumerate
/// codecs, initialise the plugin, and decode files.
///
/// # Parameters
///
/// * `host_abi_version` — the ABI version of the host (`ImageGlass`).  The major
///   version (divided by `1_000_000`) must match `IG_PLUGIN_ABI_VERSION` for
///   compatibility.
/// * `host_api` — pointer to the host API table, which provides services such
///   as logging and memory allocation.
///
/// # Safety
///
/// The caller must pass a valid `host_api` pointer that remains valid for the
/// entire lifetime of the plugin.  The returned pointer is valid for the
/// lifetime of the process.
///
/// # Returns
///
/// * A pointer to the static [`IGPluginApi`] on success.
/// * `null` if the ABI version is incompatible, `host_api` is null, or
///   initialisation fails.
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)] // struct_size is the first field; validated non-null above
pub extern "C" fn ig_plugin_get_api(
    host_abi_version: i32,
    host_api: *const IGHostApi,
) -> *const IGPluginApi {
    // Check major version compatibility (e.g., 1_000_000 → major=1).
    if host_abi_version / 1_000_000 != IG_PLUGIN_ABI_VERSION / 1_000_000 {
        return std::ptr::null();
    }

    if host_api.is_null() {
        return std::ptr::null();
    }

    // Validate the host API struct size before storing the pointer.  The
    // host allocates `IGHostApi` and sets `struct_size` to its own `sizeof`;
    // if it is smaller than ours, the host predates fields we read
    // (`abi_version`, `core`) and reading them would be out of bounds.
    // SAFETY: `host_api` was validated non-null above; `struct_size` is the
    // first field of `IGHostApi` and is present in every host version.
    if unsafe { (*host_api).struct_size } < std::mem::size_of::<IGHostApi>() as i32 {
        return std::ptr::null();
    }

    // Store the host API pointer so codec functions can access it later.
    // If `set()` fails, the value was already stored (identical pointer) —
    // this is not an error.
    let _ = HOST_API.set(HostApiPtr(host_api));

    let result = catch_unwind(|| -> *const IGPluginApi {
        ensure_initialized();
        PLUGIN_STATE
            .get()
            .map_or(std::ptr::null(), |s| std::ptr::from_ref(&s.plugin_api))
    });

    result.unwrap_or(std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host API that outlives the process (leaked) so the global
    /// `HOST_API` `OnceLock` stays valid even after this test returns —
    /// `OnceLock` cannot be reset.
    fn leaked_host_api() -> &'static IGHostApi {
        Box::leak(Box::new(IGHostApi {
            struct_size: std::mem::size_of::<IGHostApi>() as i32,
            abi_version: IG_PLUGIN_ABI_VERSION,
            core: std::ptr::null(),
        }))
    }

    /// F-001: null `host_api` must be rejected → null

    #[test]
    fn entry_point_rejects_null_host_api() {
        let api = ig_plugin_get_api(IG_PLUGIN_ABI_VERSION, std::ptr::null());
        assert!(api.is_null());
    }

    /// F-001: ABI version mismatch must be rejected → null

    #[test]
    fn entry_point_rejects_mismatched_abi_major() {
        let host = leaked_host_api();
        let api = ig_plugin_get_api(2_000_000, std::ptr::from_ref(host));
        assert!(api.is_null());
    }
    /// F-001: undersized `host_api` must be rejected → null

    #[test]
    fn entry_point_rejects_undersized_host_api() {
        let host = Box::leak(Box::new(IGHostApi {
            struct_size: 4, // undersized: only the struct_size field
            abi_version: IG_PLUGIN_ABI_VERSION,
            core: std::ptr::null(),
        }));
        let api = ig_plugin_get_api(IG_PLUGIN_ABI_VERSION, std::ptr::from_ref(host));
        assert!(api.is_null());
    }

    /// F-001, F-002: valid `host_api` must return populated plugin API table

    #[test]
    fn entry_point_returns_valid_plugin_api() {
        let host = leaked_host_api();
        let api = ig_plugin_get_api(IG_PLUGIN_ABI_VERSION, std::ptr::from_ref(host));
        assert!(!api.is_null());
        // SAFETY: non-null pointer returned by the entry point; the table
        // lives in plugin-lifetime state.
        let plugin_api = unsafe { &*api };
        assert_eq!(
            plugin_api.struct_size,
            std::mem::size_of::<IGPluginApi>() as i32
        );
        assert_eq!(plugin_api.abi_version, IG_PLUGIN_ABI_VERSION);
        assert_eq!(plugin_api.info.codec_count, 1);
        assert!(plugin_api.get_codec.is_some());
        assert!(plugin_api.initialize.is_some());
        assert!(plugin_api.shutdown.is_some());
        assert!(plugin_api.self_test.is_some());

        // The single codec (index 0) must hand back a populated table.
        let mut codec: *const IGCodecApi = std::ptr::null();
        let get_codec = plugin_api.get_codec.expect("get_codec present");
        // SAFETY: `get_codec` comes from the plugin's own table and `codec`
        // is a writable stack slot; the returned table lives for the process.
        let status = unsafe { get_codec(0, std::ptr::from_mut(&mut codec)) };
        assert_eq!(status, IGStatus::Ok);
        assert!(!codec.is_null());
        // SAFETY: non-null table returned by the plugin (see above).
        let codec_api = unsafe { &*codec };
        assert_eq!(
            codec_api.struct_size,
            std::mem::size_of::<IGCodecApi>() as i32
        );
        assert!(codec_api.get_capability.is_some());
        assert!(codec_api.can_handle_extension.is_some());
        assert!(codec_api.load_metadata.is_some());
        assert!(codec_api.decode_static_raster.is_some());
        assert!(codec_api.free_pixel_buffer.is_some());
        // Animation and encoding are unsupported — pointers must be None.
        assert!(codec_api.get_animation_info.is_none());
        assert!(codec_api.encode_static_raster.is_none());
    }

    /// F-001: `self_test` function must return Ok through the plugin API
    #[test]
    fn self_test_succeeds() {
        let host = leaked_host_api();
        let api = ig_plugin_get_api(IG_PLUGIN_ABI_VERSION, std::ptr::from_ref(host));
        assert!(!api.is_null());
        let plugin_api = unsafe { &*api };
        let self_test = plugin_api.self_test.expect("self_test present");
        // SAFETY: self_test takes no arguments and returns IGStatus.
        let status = unsafe { self_test() };
        assert_eq!(status, IGStatus::Ok);
    }

    /// F-001: UTF-16 encode/decode roundtrip must preserve content
    #[test]
    fn test_utf16_roundtrip() {
        let original = "Hello, ithmb!";
        let (buf, string_ref) = crate::types::ig_string_ref_from_str(original);
        assert!(string_ref.length > 0);
        let decoded = crate::strings::utf16_to_string(&string_ref);
        assert_eq!(decoded.as_deref(), Some(original));
        drop(buf);
    }

    /// F-002: ABI struct sizes must match C header exactly

    #[test]
    fn test_abi_struct_sizes() {
        // Layout contract from ig_plugin_abi.h (64-bit): sizes must match the
        // C structs exactly or the host's offset-based reads misalign.
        assert_eq!(std::mem::size_of::<IGCodecApi>(), 112);
        assert_eq!(std::mem::size_of::<types::IGCodecCapability>(), 104);
        assert_eq!(std::mem::size_of::<types::IGAnimationInfo>(), 16);
        assert_eq!(std::mem::size_of::<types::IGAnimationFrameInfo>(), 8);
    }
}
