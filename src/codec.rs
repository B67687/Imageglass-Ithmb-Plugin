//! Codec API implementation: capability query, extension/signature matching,
//! and metadata loading.
//!
//! Every function here is an `extern "C"` entry point referenced from the
//! `IGCodecApi` function table built in [`crate::state`].  Each one runs its
//! body inside `catch_unwind` so a panic can never unwind through the C ABI.

use std::panic::catch_unwind;

use libc::c_void;

use crate::get_host_api;
use crate::logging::Logger;
use crate::state::{CAPABILITY, PLUGIN_EXTENSIONS};
use crate::strings::utf16_to_string;
use crate::types::{IGCodecCapability, IGImageInfo, IGStatus, IGStringRef};

// ---------------------------------------------------------------------------
// Capability query
// ---------------------------------------------------------------------------

/// Returns a pointer to the plugin-allocated [`IGCodecCapability`].
///
/// The capability is allocated once during `ensure_initialized` and lives for
/// the lifetime of the plugin.  The host never fills a buffer: since SDK
/// v1.1.0 the host cannot know the capability's size beforehand, so the
/// plugin allocates it and hands back the address.
///
/// # Safety
///
/// The `caller` (`ImageGlass` `host`) must pass a writable `*mut IGCodecCapability` slot,
/// or null (rejected with `InvalidArg`). The stored pointer borrows plugin-lifetime
/// statics and must never be freed by the host.
pub(crate) unsafe extern "C" fn codec_get_capability(cap: *mut *mut IGCodecCapability) -> IGStatus {
    let result = catch_unwind(|| -> IGStatus {
        if cap.is_null() {
            return IGStatus::InvalidArg;
        }

        let Some(capability) = CAPABILITY.get() else {
            return IGStatus::Internal;
        };

        // SAFETY: `cap` was validated non-null above and points to a writable
        // `IGCodecCapability*` slot owned by the host.  The capability itself
        // is plugin-allocated and lives for the lifetime of the plugin.
        unsafe {
            *cap = std::ptr::from_ref(&capability.0).cast_mut();
        }

        IGStatus::Ok
    });

    result.unwrap_or(IGStatus::Internal)
}

// ---------------------------------------------------------------------------
// Extension / signature matching
// ---------------------------------------------------------------------------

/// Checks whether the given file extension is supported.
///
/// Performs a case-insensitive ASCII comparison against `.ithmb` and `.ipm`.
///
/// # Safety
///
/// If `ext.data` is non-null with `ext.length > 0`, it must point to `ext.length`
/// readable UTF-16 code units valid for the call. Null/empty inputs are rejected.
/// Non-ASCII code units are truncated to u8 for logging only (see F-P1).
pub(crate) unsafe extern "C" fn codec_can_handle_extension(ext: IGStringRef) -> i32 {
    // The entire body runs inside catch_unwind: the host-facing logging block
    // below can panic on a hostile extension (e.g. an absurd Length), and a
    // panic unwinding through extern "C" is UB/abort.
    let result = catch_unwind(|| -> i32 {
        if let Some(host_api) = get_host_api().filter(|a| !a.core.is_null()) {
            // Stack-convert UTF-16 extension to ASCII bytes for logging,
            // avoiding a heap allocation (String::from_utf16_lossy) on the
            // hot path.  Extensions are pure ASCII so u16→u8 is lossless.
            let ext_str: &str;
            let mut ext_buf = [0u8; 32];
            if ext.data.is_null() || ext.length <= 0 {
                ext_str = "null";
            } else {
                // SAFETY: the host guarantees `ext.data` is valid for
                // `ext.length` UTF-16 code units for the duration of the call.
                let slice = unsafe { std::slice::from_raw_parts(ext.data, ext.length as usize) };
                let n = slice.len().min(32);
                for (i, &ch) in slice[..n].iter().enumerate() {
                    ext_buf[i] = ch as u8;
                }
                // SAFETY: all bytes came from ASCII u16 values (extensions
                // are `.`, `i`, `t`, `h`, `m`, `b`, `p` only).
                ext_str = unsafe { std::str::from_utf8_unchecked(&ext_buf[..n]) };
            }
            // SAFETY: `host_api.core` was filtered non-null above and
            // `Logger::info` only calls the host's Log function.
            unsafe {
                Logger::new(host_api.core)
                    .info(&format!("ithmb-codec: can_handle_extension('{ext_str}')"));
            }
        }
        if ext.data.is_null() || ext.length <= 0 {
            return 0;
        }

        let exts = match PLUGIN_EXTENSIONS.get() {
            Some(e) => &e.0,
            None => return 0,
        };

        #[allow(clippy::cast_sign_loss)]
        // SAFETY: `ext.data` is valid for `ext.length` UTF-16 code units
        // (host guarantee, same as the logging block above).
        let input_slice = unsafe { std::slice::from_raw_parts(ext.data, ext.length as usize) };

        for known_ext in exts {
            if known_ext.length != ext.length || known_ext.data.is_null() {
                continue;
            }

            #[allow(clippy::cast_sign_loss)]
            // SAFETY: `known_ext.data` points into the read-only
            // `PLUGIN_EXTENSIONS` static and is valid for `known_ext.length`
            // code units.
            let known_slice =
                unsafe { std::slice::from_raw_parts(known_ext.data, known_ext.length as usize) };

            // Both slices contain ASCII text only (`.`, `i`, `t`, `h`, `m`, `b`, `p`).
            let eq = input_slice
                .iter()
                .zip(known_slice.iter())
                .all(|(a, b)| (*a as u8).eq_ignore_ascii_case(&(*b as u8)));

            if eq {
                return 1;
            }
        }

        0
    });

    result.unwrap_or(0)
}

/// .ithmb files have no fixed magic signature at offset 0.
/// We rely on extension matching + decode priority for selection.
///
/// # Safety
///
/// Takes no action on `_data`/`_len` (always returns 0); any pointer, including null,
/// is safe to pass.
pub(crate) unsafe extern "C" fn codec_can_handle_signature(_data: *const u8, _len: i32) -> i32 {
    0
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Reads metadata from an .ithmb file by extracting the 4-byte format prefix
/// and looking up the known dimensions from the profile database.
///
/// # Safety
///
/// `info` must be a writable `*mut IGImageInfo` slot, or null (rejected with
/// `InvalidArg`). `path` must satisfy `utf16_to_string`'s contract (see `strings.rs`).
pub(crate) unsafe extern "C" fn codec_load_metadata(
    path: IGStringRef,
    info: *mut IGImageInfo,
    _cancellation: *mut c_void,
) -> IGStatus {
    let result = catch_unwind(|| -> IGStatus {
        if info.is_null() {
            return IGStatus::InvalidArg;
        }
        let Some(path_str) = utf16_to_string(&path) else {
            return IGStatus::InvalidArg;
        };

        // Pre-size check BEFORE reading: the profile database only covers
        // thumbnail-sized images, and 8 MiB is far beyond any real .ithmb
        // payload.  Reject oversized inputs without pulling them into memory.
        let (prefix_bytes, file_size) = match crate::file_io::read_ithmb_prefix(&path_str) {
            Ok(result) => result,
            Err(status) => return status,
        };
        let prefix = i32::from_be_bytes(prefix_bytes);
        // Fast path: try device profiles (covers common device models).
        let formats = ithmb_core::device_profiles::find_formats_by_id(prefix);
        if let Some((w, h)) = formats.iter().find_map(|f| parse_dimensions(f.description)) {
            fill_image_info(info, w, h, file_size as i64);
            return IGStatus::Ok;
        }
        // Fallback: look up the prefix in the built-in ProfileDb (covers all 53 active profiles).
        let Ok(db) = ithmb_core::profile_db::ProfileDb::load_builtin() else {
            return IGStatus::Internal;
        };
        let Some(profile) = db.get(prefix) else {
            return IGStatus::NotImplemented;
        };
        fill_image_info(
            info,
            profile.display_width() as usize,
            profile.display_height() as usize,
            file_size as i64,
        );
        IGStatus::Ok
    });
    result.unwrap_or(IGStatus::Internal)
}

/// Helper: fill the standard `IGImageInfo` fields for a decoded image.
fn fill_image_info(info: *mut IGImageInfo, width: usize, height: usize, file_size: i64) {
    // SAFETY: `info` is guaranteed by the caller (`codec_load_metadata`) to be
    // non-null and to point at a host-allocated `IGImageInfo` that outlives
    // this call.
    unsafe {
        (*info).width = width as i32;
        (*info).height = height as i32;
        (*info).pixel_format = 1; // IGPixelFormat::Bgra8Unorm
        (*info).has_alpha = 1;
        (*info).hdr_transfer_fn = 0; // IGHdrTransferFn::None
        (*info).color_space = 1; // IGColorSpace::Srgb
        (*info).orientation = 0; // EXIF 1..8; 0 = unknown
        (*info).frame_count = 1;
        (*info).file_size_bytes = file_size;
        (*info).icc_profile_data = std::ptr::null();
        (*info).icc_profile_size = 0;
    }
}

/// Parse a dimensions string (e.g. `"320×240"`) from a `DeviceFormatInfo` description.
fn parse_dimensions(desc: &str) -> Option<(usize, usize)> {
    // Descriptions use × (unicode multiplication sign). Height may be followed by comma/space.
    let cross = desc.find('×')?;
    let width: usize = desc[..cross].trim().parse().ok()?;
    let rest = &desc[cross + '×'.len_utf8()..];
    let height_digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let height: usize = height_digits.parse().ok()?;
    Some((width, height))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ensure_initialized;

    fn zero_image_info() -> IGImageInfo {
        IGImageInfo {
            width: 0,
            height: 0,
            pixel_format: 0,
            has_alpha: 0,
            hdr_transfer_fn: 0,
            color_space: 0,
            orientation: 0,
            frame_count: 0,
            file_size_bytes: 0,
            icc_profile_data: std::ptr::null(),
            icc_profile_size: 0,
        }
    }

    /// F-005: `parse_dimensions` must extract width×height from profile description strings

    #[test]
    fn test_parse_dimensions() {
        assert_eq!(
            parse_dimensions("720×480 YUV422 interlaced full-screen"),
            Some((720, 480))
        );
        assert_eq!(parse_dimensions("320×240 RGB565 photo"), Some((320, 240)));
        assert_eq!(
            parse_dimensions("128×128 RGB565 cover art"),
            Some((128, 128))
        );
        assert_eq!(parse_dimensions("100×100 RGB565"), Some((100, 100)));
        assert_eq!(
            parse_dimensions("720×480 YCbCr420 padded"),
            Some((720, 480))
        );
        assert_eq!(parse_dimensions("56×56 RGB565"), Some((56, 56)));
        assert_eq!(parse_dimensions(""), None);
        // Comma-formatted descriptions (e.g. from find_formats_by_id)
        assert_eq!(
            parse_dimensions("320×320, RGB555, 204800 bytes/frame"),
            Some((320, 320))
        );
        assert_eq!(
            parse_dimensions("720×480, YCbCr420, 691200 bytes/frame"),
            Some((720, 480))
        );
    }

    /// F-003: null output slot must be rejected → `InvalidArg`

    #[test]
    fn capability_rejects_null_output_slot() {
        ensure_initialized();
        // SAFETY: a null `*mut *mut` slot is deliberately passed; the function
        // validates it before any write.
        let status = unsafe { codec_get_capability(std::ptr::null_mut()) };
        assert_eq!(status, IGStatus::InvalidArg);
    }

    /// F-003: valid output must populate full v1.1.0 capability fields

    #[test]
    fn capability_reports_v110_contract() {
        ensure_initialized();
        let mut cap: *mut IGCodecCapability = std::ptr::null_mut();
        // SAFETY: `cap` points to a writable stack slot for the duration of
        // the call; on success the plugin stores a plugin-lifetime pointer.
        let status = unsafe { codec_get_capability(std::ptr::from_mut(&mut cap)) };
        assert_eq!(status, IGStatus::Ok);
        assert!(!cap.is_null());
        // SAFETY: non-null capability pointer returned by the plugin; it lives
        // for the process lifetime (see `state::CAPABILITY`).
        let capability = unsafe { &*cap };
        assert_eq!(
            capability.struct_size,
            std::mem::size_of::<IGCodecCapability>() as i32
        );
        assert_eq!(capability.supports_static_raster_decoding, 1);
        assert_eq!(capability.supports_animation_decoding, 0);
        assert_eq!(capability.supports_static_raster_encoding, 0);
        assert_eq!(capability.decode_extension_count, 2);
        assert_eq!(capability.encode_extension_count, 0);
        assert!(capability.encode_extensions.is_null());
        assert_eq!(capability.metadata_priority, 200);
        assert_eq!(capability.decode_priority, 200);
    }

    /// F-004: .ithmb and .ipm extensions must match

    #[test]
    fn can_handle_extension_matches_known_extensions() {
        ensure_initialized();
        let (ithmb, ithmb_ref) = crate::types::ig_string_ref_from_str(".ithmb");
        let (ipm, ipm_ref) = crate::types::ig_string_ref_from_str(".ipm");
        // SAFETY: the string refs point into live UTF-16 buffers for the call.
        let ithmb_hit = unsafe { codec_can_handle_extension(ithmb_ref) };
        let ipm_hit = unsafe { codec_can_handle_extension(ipm_ref) };
        assert_eq!(ithmb_hit, 1);
        assert_eq!(ipm_hit, 1);
        // Keep buffers alive until after the FFI calls.
        drop(ithmb);
        drop(ipm);
    }

    /// F-004: extension matching must be case-insensitive ASCII

    #[test]
    fn can_handle_extension_is_case_insensitive() {
        ensure_initialized();
        // The matcher compares ASCII case-insensitively but requires an exact
        // code-unit length, so whitespace variants (".IT HMB") cannot match.
        let cases: [(&str, i32); 4] = [
            (".ITHMB", 1),
            (".iThMb", 1),
            (".IPM", 1),
            (".IT HMB", 0), // length 7 != 6 → no match
        ];
        for (text, expected) in cases {
            let (buf, string_ref) = crate::types::ig_string_ref_from_str(text);
            // SAFETY: `string_ref` points into the live `buf` for the call.
            let result = unsafe { codec_can_handle_extension(string_ref) };
            assert_eq!(result, expected, "extension {text:?}");
            drop(buf);
        }
    }

    /// F-004: unknown extensions and empty strings must be rejected

    #[test]
    fn can_handle_extension_rejects_unmatched_and_empty() {
        ensure_initialized();
        let (jpg, jpg_ref) = crate::types::ig_string_ref_from_str(".jpg");
        // SAFETY: `jpg_ref` points into the live `jpg` buffer for the call.
        assert_eq!(unsafe { codec_can_handle_extension(jpg_ref) }, 0);
        drop(jpg);

        // Null data pointer / zero length → 0.
        let null_ref = IGStringRef {
            data: std::ptr::null(),
            length: 0,
        };
        // SAFETY: a null ref is handled gracefully and rejected.
        assert_eq!(unsafe { codec_can_handle_extension(null_ref) }, 0);

        let (_empty_buf, empty_ref) = crate::types::ig_string_ref_from_str("");
        // SAFETY: a zero-length ref is handled gracefully and rejected.
        assert_eq!(unsafe { codec_can_handle_extension(empty_ref) }, 0);
    }

    /// F-005: null info pointer must be rejected → `InvalidArg`

    #[test]
    fn load_metadata_rejects_null_info() {
        ensure_initialized();
        let (path, path_ref) = crate::types::ig_string_ref_from_str("/nonexistent.ithmb");
        // SAFETY: a null info slot is deliberately passed; validated first.
        let status =
            unsafe { codec_load_metadata(path_ref, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(status, IGStatus::InvalidArg);
        drop(path);
    }

    /// F-005: missing file must return `IoError`

    #[test]
    fn load_metadata_missing_file_is_io_error() {
        ensure_initialized();
        let (path, path_ref) =
            crate::types::ig_string_ref_from_str("/nonexistent/definitely-missing.ithmb");
        let mut info = zero_image_info();
        // SAFETY: `info` points to a writable stack slot; `path_ref` points
        // into the live `path` buffer.  The file does not exist → IoError.
        let status = unsafe {
            codec_load_metadata(
                path_ref,
                std::ptr::from_mut(&mut info),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, IGStatus::IoError);
        drop(path);
    }

    /// F-005: known profile prefix must resolve via `device_profiles` fast path

    #[test]
    fn load_metadata_reads_known_profile_fixture() {
        ensure_initialized();
        // Prefix 1024 (0x0000_0400) is a known device-format id
        // ("320×240 RGB565 photo"), so metadata must resolve via the fast
        // device-profile path.
        let path = std::env::temp_dir().join(format!(
            "ithmb-cabi-meta-{}-{}.ithmb",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        std::fs::write(&path, [0x00, 0x00, 0x04, 0x00]).expect("write fixture");
        let (path_buf, path_ref) =
            crate::types::ig_string_ref_from_str(path.to_str().expect("utf8 temp path"));
        let mut info = zero_image_info();
        // SAFETY: `info` points to a writable stack slot; `path_ref` points
        // into the live `path_buf`; `path` exists on disk.
        let status = unsafe {
            codec_load_metadata(
                path_ref,
                std::ptr::from_mut(&mut info),
                std::ptr::null_mut(),
            )
        };
        drop(path_buf);
        std::fs::remove_file(&path).ok();
        assert_eq!(status, IGStatus::Ok);
        assert_eq!(info.width, 320);
        assert_eq!(info.height, 240);
        assert_eq!(info.file_size_bytes, 4);
        assert_eq!(info.pixel_format, 1); // IGPixelFormat::Bgra8Unorm
        assert_eq!(info.has_alpha, 1);
    }
}
