//! Plugin state: static storage, string buffers, and ABI function tables.
//!
//! All plugin-lifetime data lives here behind `OnceLock`s so that the raw
//! pointers handed to the host (string refs, capability, function tables)
//! remain stable for the process lifetime:
//!
//! 1. raw pointers into the `Vec` heap-buffers are stable after init, and
//! 2. the plugin is lazily initialized on first access.

use std::sync::OnceLock;

use crate::buffer_registry::BufferRegistry;
use crate::codec::{
    codec_can_handle_extension, codec_can_handle_signature, codec_get_capability,
    codec_load_metadata,
};
use crate::decode::{codec_decode_static_raster, codec_free_pixel_buffer};
use crate::strings::encode_utf16;
use crate::types::{
    IGCodecApi, IGCodecCapability, IGHostApi, IGPluginApi, IGPluginInfo, IGStringRef,
    ig_string_ref_null,
};
use crate::{
    IG_PLUGIN_ABI_VERSION, plugin_get_codec, plugin_initialize, plugin_self_test, plugin_shutdown,
};

// ---------------------------------------------------------------------------
// Extensions array
// ---------------------------------------------------------------------------

/// Wrapper around the extensions pointer array for static storage.
///
/// # Safety
///
/// The referenced data is in the read-only data section and never changes
/// after initialisation.  `IGStringRef` contains `*const u16` which is
/// neither `Send` nor `Sync` by default, but the pointed-to data is
/// immutable and lives for the program lifetime.
#[repr(transparent)]
pub(crate) struct ExtensionsArray(pub(crate) [IGStringRef; 2]);

// SAFETY: `ExtensionsArray` only stores pointers to const data in the
// binary's read-only section — they never dangle or mutate.
unsafe impl Send for ExtensionsArray {}
unsafe impl Sync for ExtensionsArray {}

pub(crate) static PLUGIN_EXTENSIONS: OnceLock<ExtensionsArray> = OnceLock::new();

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct PluginState {
    // UTF-16 string buffers — IGStringRef.data fields point into these
    // (heap-allocated; .as_ptr() is stable after OnceLock init).  The
    // fields are "dead" from Rust's perspective but MUST stay alive for
    // the raw pointers in `plugin_api` / capability references to remain
    // valid.
    plugin_id: Vec<u16>,
    plugin_name: Vec<u16>,
    plugin_version: Vec<u16>,
    cap_name: Vec<u16>,

    // ABI function tables (reference the string buffers above).
    pub(crate) codec_api: IGCodecApi,
    pub(crate) plugin_api: IGPluginApi,
}

// SAFETY: PluginState is only stored in a OnceLock and accessed immutably
// after initialization.  All raw pointers within reference either external
// statics (PLUGIN_EXTENSIONS) or heap-allocated Vec buffers owned by the
// state itself — both of which are stable for the program lifetime.
unsafe impl Send for PluginState {}
unsafe impl Sync for PluginState {}

pub(crate) static PLUGIN_STATE: OnceLock<PluginState> = OnceLock::new();

/// Wrapper around the codec capability for static storage.
///
/// # Safety
///
/// All raw pointers inside point at plugin-lifetime data (the `PLUGIN_STATE`
/// string buffers and the `PLUGIN_EXTENSIONS` static) and are only read.
#[repr(transparent)]
pub(crate) struct CodecCapabilityStorage(pub(crate) IGCodecCapability);

// SAFETY: all pointers stored inside reference plugin-lifetime immutable
// data — they never dangle and the pointed-to data never mutates.
unsafe impl Send for CodecCapabilityStorage {}
unsafe impl Sync for CodecCapabilityStorage {}

/// Plugin-lifetime codec capability, returned by pointer from
/// `codec_get_capability`.  Built after `PLUGIN_STATE` so its string refs
/// can point into the stable `PluginState` buffers.
pub(crate) static CAPABILITY: OnceLock<CodecCapabilityStorage> = OnceLock::new();

// ---------------------------------------------------------------------------
// Host API pointer
// ---------------------------------------------------------------------------

pub(crate) struct HostApiPtr(pub(crate) *const IGHostApi);

// SAFETY: The host API pointer is stored during `ig_plugin_get_api()` and is
// valid for the entire lifetime of the plugin.  Access is read-only after
// init.
unsafe impl Send for HostApiPtr {}
unsafe impl Sync for HostApiPtr {}

pub(crate) static HOST_API: OnceLock<HostApiPtr> = OnceLock::new();

/// Global registry of live pixel-buffer allocations.
pub(crate) static BUFFER_REGISTRY: OnceLock<BufferRegistry> = OnceLock::new();

// F-P3 FIX: these were `const` inside the closure below, relying on implicit
// const-promotion for process-lifetime pointers. `static` makes the lifetime
// explicit — a future non-const-compatible edit can no longer silently
// produce a dangling `IGStringRef.data`.
static EXT_ITHMB_DATA: [u16; 6] = [
    b'.' as u16,
    b'i' as u16,
    b't' as u16,
    b'h' as u16,
    b'm' as u16,
    b'b' as u16,
];
static EXT_IPM_DATA: [u16; 4] = [b'.' as u16, b'i' as u16, b'p' as u16, b'm' as u16];

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Ensures all plugin state is initialized.  Called by `ig_plugin_get_api`.
pub(crate) fn ensure_initialized() {
    // 1. Extensions array (static data — never moves, never freed).
    let _ = PLUGIN_EXTENSIONS.get_or_init(|| {
        // `EXT_*_DATA` are `static`: valid for the process lifetime (see F-P3 above).

        ExtensionsArray([
            IGStringRef {
                data: EXT_ITHMB_DATA.as_ptr(),
                length: EXT_ITHMB_DATA.len() as i32,
            },
            IGStringRef {
                data: EXT_IPM_DATA.as_ptr(),
                length: EXT_IPM_DATA.len() as i32,
            },
        ])
    });

    // 2. Plugin state (all other string buffers + ABI tables).
    let _ = PLUGIN_STATE.get_or_init(|| {
        let plugin_id = encode_utf16("ithmb-codec");
        let plugin_name = encode_utf16("iThmb Codec");
        let plugin_version = encode_utf16(env!("CARGO_PKG_VERSION"));
        let cap_name = encode_utf16("iThmb Codec");

        let codec_api = IGCodecApi {
            struct_size: std::mem::size_of::<IGCodecApi>() as i32,
            get_capability: Some(codec_get_capability as _),
            can_handle_extension: Some(codec_can_handle_extension as _),
            can_handle_signature: Some(codec_can_handle_signature as _),
            load_metadata: Some(codec_load_metadata as _),
            decode_static_raster: Some(codec_decode_static_raster as _),
            free_pixel_buffer: Some(codec_free_pixel_buffer as _),
            // Animation not supported — set all animation pointers to None.
            get_animation_info: None,
            free_animation_info: None,
            decode_animation_frame: None,
            // Encoding not supported — set all encode pointers to None.
            encode_static_raster: None,
            begin_encode_multi_frame: None,
            encode_frame: None,
            end_encode_multi_frame: None,
        };

        let plugin_api = IGPluginApi {
            struct_size: std::mem::size_of::<IGPluginApi>() as i32,
            abi_version: IG_PLUGIN_ABI_VERSION,
            info: IGPluginInfo {
                plugin_id: IGStringRef {
                    data: plugin_id.as_ptr(),
                    length: plugin_id.len() as i32,
                },
                name: IGStringRef {
                    data: plugin_name.as_ptr(),
                    length: plugin_name.len() as i32,
                },
                version: IGStringRef {
                    data: plugin_version.as_ptr(),
                    length: plugin_version.len() as i32,
                },
                abi_version: IG_PLUGIN_ABI_VERSION,
                codec_count: 1,
            },
            get_codec: Some(plugin_get_codec as _),
            initialize: Some(plugin_initialize as _),
            shutdown: Some(plugin_shutdown as _),
            self_test: Some(plugin_self_test as _),
        };

        PluginState {
            plugin_id,
            plugin_name,
            plugin_version,
            cap_name,
            codec_api,
            plugin_api,
        }
    });

    // 3. Codec capability (plugin-allocated; its string refs point into the
    //    PLUGIN_STATE buffers and the PLUGIN_EXTENSIONS static — all stable
    //    after initialization).
    let _ = CAPABILITY.get_or_init(build_capability);

    // 4. Buffer registry (live pixel-buffer tracking).
    let _ = BUFFER_REGISTRY.get_or_init(BufferRegistry::new);
}

/// Builds the plugin-lifetime codec capability.
///
/// Called from `ensure_initialized` AFTER `PLUGIN_EXTENSIONS` and
/// `PLUGIN_STATE` are initialized, so its string refs can point into the
/// stable `PluginState` buffers.
fn build_capability() -> CodecCapabilityStorage {
    let state = PLUGIN_STATE.get();
    let extensions_ptr = PLUGIN_EXTENSIONS
        .get()
        .map_or(std::ptr::null(), |e| e.0.as_ptr());

    // `PLUGIN_STATE` is always initialized before `CAPABILITY` by
    // `ensure_initialized`; if it is ever absent, fall back to null string
    // refs rather than panicking (a panic would abort the host process).
    let (codec_id, codec_name) = match state {
        Some(s) => (
            IGStringRef {
                data: s.plugin_id.as_ptr(),
                length: s.plugin_id.len() as i32,
            },
            IGStringRef {
                data: s.cap_name.as_ptr(),
                length: s.cap_name.len() as i32,
            },
        ),
        None => (ig_string_ref_null(), ig_string_ref_null()),
    };

    CodecCapabilityStorage(IGCodecCapability {
        struct_size: std::mem::size_of::<IGCodecCapability>() as i32,
        codec_id,
        codec_name,
        metadata_priority: 200,
        decode_priority: 200,
        supports_metadata: 1,
        supports_color_profiles: 0,
        supports_static_raster_decoding: 1,
        supports_animation_decoding: 0,
        decode_extension_count: 2,
        decode_extensions: extensions_ptr,
        supports_static_raster_encoding: 0,
        supports_multi_frame_encoding: 0,
        encode_priority: 0,
        encode_extension_count: 0,
        encode_extensions: std::ptr::null(),
    })
}
