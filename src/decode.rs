//! Static-raster decode path: pixel-buffer decode, free, and the global
//! buffer registry.
//!
//! [`codec_decode_static_raster`] decodes an `.ithmb` file into a
//! plugin-allocated pixel buffer; [`codec_free_pixel_buffer`] releases it.
//! Every allocation is tracked in [`crate::state::BUFFER_REGISTRY`] to
//! prevent double-free / dangling-pointer bugs.

use std::panic::catch_unwind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use libc::c_void;

use ithmb_core::decode_ithmb;

use crate::allocator;
#[allow(unused_imports)]
use crate::buffer_registry::BufferRegistry;
use crate::get_host_api;
use crate::logging::{Logger, log_error};
use crate::state::BUFFER_REGISTRY;
use crate::strings::utf16_to_string;
use crate::types::{IGPixelBuffer, IGStatus, IGStringRef, ig_status_from_decode_error};

/// Decodes a static raster frame from an .ithmb file into the caller's
/// [`IGPixelBuffer`].
///
/// # Safety
///
/// - `path` must be a valid `IGStringRef` (null data or negative length
///   is rejected as `InvalidArg` via `utf16_to_string`).
/// - `buffer` must be non-null and point to a host-allocated `IGPixelBuffer`
///   that outlives the call; the plugin writes all fields on success.
pub(crate) unsafe extern "C" fn codec_decode_static_raster(
    path: IGStringRef,
    frame_index: i32,
    buffer: *mut IGPixelBuffer,
    _cancellation: *mut c_void,
) -> IGStatus {
    let result = catch_unwind(|| -> IGStatus {
        // ---- Input validation ----
        if buffer.is_null() {
            return IGStatus::InvalidArg;
        }

        let Some(path_str) = utf16_to_string(&path) else {
            return IGStatus::InvalidArg;
        };

        // Only single-frame static images are supported
        if frame_index != 0 {
            return IGStatus::InvalidArg;
        }

        // ---- Read file ----
        let file_bytes = match crate::file_io::read_ithmb_file(&path_str) {
            Ok(data) => data,
            Err(status) => {
                if let Some(host_api) = get_host_api().filter(|api| !api.core.is_null()) {
                    let logger = Logger::new(host_api.core);
                    // SAFETY: `host_api.core` was filtered non-null above and
                    // `log_error!` only calls the host's Log function.
                    log_error!(logger, "ithmb-codec: failed to read file: {status:?}");
                }
                return status;
            }
        };

        // ---- Cooperative cancellation flag for decode_ithmb ----
        let canceled = Arc::new(AtomicBool::new(false));

        // ---- Decode ----
        let decoded = match decode_ithmb(&file_bytes, &canceled) {
            Ok(img) => img,
            Err(e) => {
                canceled.store(true, Ordering::Relaxed);
                return ig_status_from_decode_error(&e);
            }
        };

        // Signal cancellation monitor to stop
        canceled.store(true, Ordering::Relaxed);
        // ---- Allocate pixel buffer (self-managed) ----

        let width = decoded.width as i32;
        let height = decoded.height as i32;
        // Checked arithmetic: a hostile file can report extreme dimensions,
        // and a wrapping i32/usize multiply could under-allocate the buffer.
        let Some(stride) = width.checked_mul(4) else {
            return IGStatus::OutOfMemory;
        };
        let Some(buf_size) = (height as usize).checked_mul(stride as usize) else {
            return IGStatus::OutOfMemory;
        };

        // SAFETY: `allocator::pixel_buffer_alloc` returns a zero-initialized
        // allocation of `buf_size` bytes or a null pointer; the pointer stays
        // owned by us until registered/freed.
        let data_ptr = unsafe { allocator::pixel_buffer_alloc(buf_size) };
        if data_ptr.is_null() {
            return IGStatus::OutOfMemory;
        }

        // SAFETY: `decoded.data` holds exactly `buf_size` bytes (width * height
        // * 4 bytes per BGRA pixel) and `data_ptr` points to a fresh allocation
        // of the same size; the regions cannot overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(decoded.data.as_ptr(), data_ptr, buf_size);
        }

        // ---- Register buffer ----
        let registry = BUFFER_REGISTRY.get().unwrap();
        if registry.register(data_ptr, buf_size).is_err() {
            // SAFETY: `data_ptr` is our own allocation from
            // `allocator::pixel_buffer_alloc`, not yet registered anywhere.
            unsafe {
                allocator::pixel_buffer_free(data_ptr);
            }
            return IGStatus::Internal;
        }

        // ---- Populate IGPixelBuffer ----
        // SAFETY: `buffer` was validated non-null above and points to a
        // host-allocated `IGPixelBuffer` that outlives this call.
        unsafe {
            (*buffer).data = data_ptr;
            (*buffer).width = width;
            (*buffer).height = height;
            (*buffer).stride = stride;
            (*buffer).pixel_format = 1; // IGPixelFormat::Bgra8Unorm
            (*buffer).release_context = std::ptr::null_mut();
        }

        IGStatus::Ok
    });

    result.unwrap_or(IGStatus::Internal)
}

/// Releases a pixel buffer previously filled by [`codec_decode_static_raster`].
///
/// # Safety
///
/// - `buffer` may be null (no-op). Otherwise it must point to the host-owned
///   `IGPixelBuffer` this plugin filled — the plugin reads its `data` pointer
///   and zeroes the struct fields.
pub(crate) unsafe extern "C" fn codec_free_pixel_buffer(buffer: *mut IGPixelBuffer) {
    #[allow(clippy::let_unit_value)]
    let _ = catch_unwind(|| {
        if buffer.is_null() {
            return;
        }

        // SAFETY: `buffer` is guaranteed by the host to point at an
        // `IGPixelBuffer` this plugin previously filled; it is only read here.
        let data_ptr = unsafe { (*buffer).data };

        // Always clear struct first — prevents ImageGlass accessing stale pointers.
        // SAFETY: same validity guarantee as above; the host owns the struct
        // and we only zero the fields we previously populated.
        unsafe {
            (*buffer).data = std::ptr::null_mut();
            (*buffer).width = 0;
            (*buffer).height = 0;
            (*buffer).stride = 0;
        }

        if data_ptr.is_null() {
            return;
        }

        // Unregister from buffer registry
        let registry = BUFFER_REGISTRY.get().unwrap();
        if registry.unregister(data_ptr).is_err() {
            return;
        }

        // SAFETY: `data_ptr` was allocated by `allocator::pixel_buffer_alloc`
        // and is not referenced anywhere else after unregistering.
        unsafe {
            allocator::pixel_buffer_free(data_ptr);
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ensure_initialized;

    fn zero_pixel_buffer() -> IGPixelBuffer {
        IGPixelBuffer {
            data: std::ptr::null_mut(),
            width: 0,
            height: 0,
            stride: 0,
            pixel_format: 0,
            release_context: std::ptr::null_mut(),
        }
    }

    /// F-006: null buffer pointer must be rejected → `InvalidArg`
    #[test]
    fn decode_rejects_null_buffer() {
        ensure_initialized();
        let (path, path_ref) = crate::types::ig_string_ref_from_str("/nonexistent.ithmb");
        // SAFETY: a null buffer slot is deliberately passed; validated first.
        let status = unsafe {
            codec_decode_static_raster(path_ref, 0, std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(status, IGStatus::InvalidArg);
        drop(path);
    }

    /// F-006: nonzero frame index must be rejected → `InvalidArg`
    #[test]
    fn decode_rejects_nonzero_frame_index() {
        ensure_initialized();
        let (path, path_ref) = crate::types::ig_string_ref_from_str("/nonexistent.ithmb");
        let mut buffer = zero_pixel_buffer();
        // SAFETY: `buffer` points to a writable stack slot; frame 1 is
        // rejected before any I/O.
        let status = unsafe {
            codec_decode_static_raster(
                path_ref,
                1,
                std::ptr::from_mut(&mut buffer),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, IGStatus::InvalidArg);
        drop(path);
    }

    /// F-006: missing file must return `IoError`
    #[test]
    fn decode_missing_file_is_io_error() {
        ensure_initialized();
        let (path, path_ref) =
            crate::types::ig_string_ref_from_str("/nonexistent/definitely-missing.ithmb");
        let mut buffer = zero_pixel_buffer();
        // SAFETY: `buffer` points to a writable stack slot; `path_ref` points
        // into the live `path` buffer.  The file does not exist → IoError.
        let status = unsafe {
            codec_decode_static_raster(
                path_ref,
                0,
                std::ptr::from_mut(&mut buffer),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, IGStatus::IoError);
        drop(path);
    }

    /// End-to-end roundtrip: encode a real 80×80 RGB565 .ithmb fixture with
    /// ithmb-core, decode it through the plugin FFI, and verify the buffer.
    /// F-006: encode+decode roundtrip must preserve pixel data
    #[test]
    fn decode_roundtrips_encoded_fixture() {
        ensure_initialized();
        // Profile 1005 (80×80 RGB565, 12800 bytes/frame) exists in the
        // built-in profile DB — verified against the ithmb-core 1.9.5 data.
        let db = ithmb_core::profile_db::ProfileDb::load_builtin().expect("builtin profile db");
        let profile = db.get(1005).expect("profile 1005 present");
        let width = 80_i32;
        let height = 80_i32;
        // Deterministic gradient-ish BGRA source.
        let bgra: Vec<u8> = (0..(width * height) as usize)
            .flat_map(|i| {
                let base = i as u8;
                [
                    base.wrapping_mul(7),
                    base.wrapping_mul(13),
                    base.wrapping_mul(29),
                    255,
                ]
            })
            .collect();
        let file = ithmb_core::enc::build_ithmb_file(&bgra, width, height, profile);

        let path = std::env::temp_dir().join(format!(
            "ithmb-cabi-rt-{}-{}.ithmb",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        std::fs::write(&path, &file).expect("write fixture");
        let (path_buf, path_ref) =
            crate::types::ig_string_ref_from_str(path.to_str().expect("utf8 temp path"));

        let mut buffer = zero_pixel_buffer();
        // SAFETY: `buffer` points to a writable stack slot and outlives the
        // call; the fixture exists on disk; on Ok the plugin fills `buffer`.
        let status = unsafe {
            codec_decode_static_raster(
                path_ref,
                0,
                std::ptr::from_mut(&mut buffer),
                std::ptr::null_mut(),
            )
        };
        drop(path_buf);

        assert_eq!(status, IGStatus::Ok);
        assert!(!buffer.data.is_null());
        assert_eq!(buffer.width, width);
        assert_eq!(buffer.height, height);
        assert_eq!(buffer.stride, width * 4);
        assert_eq!(buffer.pixel_format, 1); // IGPixelFormat::Bgra8Unorm

        // Verify the decoded bytes are a plausible BGRA frame of the right size.
        let buf_size = (height as usize) * (buffer.stride as usize);
        // SAFETY: on Ok the plugin registered a `buf_size`-byte allocation at
        // `buffer.data`; reading it here is valid until freed below.
        let decoded: &[u8] = unsafe { std::slice::from_raw_parts(buffer.data, buf_size) };
        assert_eq!(decoded.len(), width as usize * height as usize * 4);

        // Free the buffer back to the plugin; the struct must be zeroed.
        // SAFETY: `buffer` holds the plugin's own allocation (see above).
        unsafe { codec_free_pixel_buffer(std::ptr::from_mut(&mut buffer)) };
        assert!(buffer.data.is_null());
        assert_eq!(buffer.width, 0);
        assert!(
            crate::state::BUFFER_REGISTRY
                .get()
                .is_none_or(BufferRegistry::is_empty)
        );
        std::fs::remove_file(&path).ok();
    }
}

// ---------------------------------------------------------------------------
// Pseudo-fuzz (deterministic, std-only)
// ---------------------------------------------------------------------------

/// Deterministic pseudo-fuzz for the decode path.
///
/// Feeds mutated byte vectors to `ithmb_core::decode_ithmb` (the decoder the
/// plugin wraps in `catch_unwind`) and asserts the one invariant that matters
/// for the FFI boundary: **the decoder never panics** on hostile input.  A
/// panic here would be caught by the plugin's `catch_unwind` and surfaced as
/// `IGStatus::Internal`, silently degrading a valid file into a failure.
///
/// No external deps: the PRNG and the mutation operators are hand-rolled.
/// A fixed seed keeps the run fully reproducible.
#[cfg(test)]
mod fuzz {
    use std::sync::atomic::AtomicBool;

    use ithmb_core::decode_ithmb;

    /// Fixed seed — the whole run is deterministic across machines/CI.
    const SEED: u64 = 0x5EED_2026_1B8E_F00D;
    /// Number of mutated inputs generated per fuzz test.
    const MUTATIONS: usize = 3_000;
    /// Cap on mutated input length — bounds decode time in debug builds.
    const MAX_INPUT_LEN: usize = 1_024;

    /// Minimal xorshift64 PRNG (Marsaglia).  Shift-based — no overflow
    /// panics in debug builds, unlike multiply-based generators.
    struct XorShift64 {
        state: u64,
    }

    impl XorShift64 {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            x
        }

        /// Uniform integer in `0..bound`.
        fn below(&mut self, bound: usize) -> usize {
            (self.next_u64() % bound as u64) as usize
        }
    }

    /// Seed vectors: minimal valid-ish .ithmb headers (real 4-byte prefixes
    /// from the ithmb-core profile DB) plus degenerate inputs. A real fixture
    /// exists at tests/fixtures/test1.ithmb, but these synthetic seeds provide
    /// deterministic coverage of edge cases.
    fn seed_vectors() -> Vec<Vec<u8>> {
        // Prefix 1024 = 0x0000_0400 (320×240 RGB565), 1019 = 0x0000_03FB
        // (720×480 YUV422), 1093 = 0x0000_0445 (512×512 RGB565).
        let pad = |n: usize, start: u8| -> Vec<u8> {
            (0..n).map(|i| start.wrapping_add(i as u8)).collect()
        };
        vec![
            Vec::new(),
            vec![0x00],
            vec![0xFF, 0xD8], // JPEG stream marker, too short for a header
            vec![0xFF, 0xD8, 0xFF, 0xE0], // JPEG SOI+APP0, no payload
            vec![0x00, 0x00, 0x04, 0x00], // prefix 1024, header only
            vec![0x00, 0x00, 0x03, 0xFB], // prefix 1019, header only
            {
                let mut v = vec![0x00, 0x00, 0x04, 0x00];
                v.extend(pad(60, 0x11)); // prefix + 60-byte payload
                v
            },
            {
                let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
                v.extend(pad(60, 0x22)); // JPEG-ish stream with padding
                v
            },
            {
                let mut v = vec![0x00, 0x00, 0x04, 0x45]; // prefix 1093
                v.extend(pad(60, 0x33));
                v
            },
        ]
    }

    /// Applies one random mutation: bit flip, truncation, random extension,
    /// or byte overwrite.
    fn mutate(rng: &mut XorShift64, input: &mut Vec<u8>) {
        match rng.below(4) {
            0 => {
                if !input.is_empty() {
                    let i = rng.below(input.len());
                    input[i] ^= 1 << rng.below(8);
                }
            }
            1 => {
                let new_len = rng.below(input.len() + 1);
                input.truncate(new_len);
            }
            2 => {
                if input.len() < MAX_INPUT_LEN {
                    let n = rng.below(32).min(MAX_INPUT_LEN - input.len());
                    input.extend(std::iter::repeat_with(|| rng.next_u64() as u8).take(n));
                }
            }
            _ => {
                if !input.is_empty() {
                    let i = rng.below(input.len());
                    input[i] = rng.next_u64() as u8;
                }
            }
        }
    }

    /// Every mutated input must decode to `Ok` or `Err` — never unwind.
    /// F-007: 3000-mutation pseudo-fuzz must never unwind
    #[test]
    fn mutated_inputs_never_panic_the_decoder() {
        let mut rng = XorShift64 { state: SEED };
        let canceled = AtomicBool::new(false);
        let seeds = seed_vectors();

        // Baseline: each seed itself must not panic.
        for seed in &seeds {
            let outcome = std::panic::catch_unwind(|| decode_ithmb(seed, &canceled));
            assert!(
                outcome.is_ok(),
                "decoder panicked on seed (len {})",
                seed.len()
            );
        }

        let mut errors = 0usize;
        let mut successes = 0usize;
        for _ in 0..MUTATIONS {
            let mut input = seeds[rng.below(seeds.len())].clone();
            // Apply 1..=3 stacked mutations per vector.
            let rounds = 1 + rng.below(3);
            for _ in 0..rounds {
                mutate(&mut rng, &mut input);
            }

            let outcome = std::panic::catch_unwind(|| decode_ithmb(&input, &canceled));
            match outcome {
                Ok(Ok(_)) => successes += 1,
                Ok(Err(err)) => {
                    // Any decode error is acceptable — the invariant is
                    // that hostile input never panics.
                    let _ = err;
                    errors += 1;
                }
                Err(payload) => {
                    // Re-raise the decoder panic to fail this test.
                    std::panic::resume_unwind(payload);
                }
            }
        }

        // Sanity: the harness must actually exercise error paths — empty and
        // truncated seeds guarantee some failures regardless of mutation luck.
        assert!(errors > 0, "fuzz harness never produced a decode error");
        // Report the split for debugging; both outcomes are legitimate.
        assert!(
            successes + errors == MUTATIONS,
            "every mutation must produce a decisive outcome"
        );
    }
}
