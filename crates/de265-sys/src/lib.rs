//! Raw bindings to the parts of libde265's C API the game uses.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

pub type de265_error = c_int;
pub type de265_PTS = i64;
pub type de265_decoder_context = c_void;

#[repr(C)]
pub struct de265_image {
    _private: [u8; 0],
}

pub const DE265_OK: de265_error = 0;
pub const DE265_ERROR_IMAGE_BUFFER_FULL: de265_error = 9;
pub const DE265_ERROR_WAITING_FOR_INPUT_DATA: de265_error = 13;

/// `enum de265_chroma`
pub const DE265_CHROMA_MONO: c_int = 0;
pub const DE265_CHROMA_420: c_int = 1;
pub const DE265_CHROMA_422: c_int = 2;
pub const DE265_CHROMA_444: c_int = 3;

extern "C" {
    pub fn de265_get_error_text(err: de265_error) -> *const c_char;
    pub fn de265_isOK(err: de265_error) -> c_int;
    pub fn de265_set_verbosity(level: c_int);

    pub fn de265_new_decoder() -> *mut de265_decoder_context;
    pub fn de265_start_worker_threads(ctx: *mut de265_decoder_context, number_of_threads: c_int) -> de265_error;
    pub fn de265_free_decoder(ctx: *mut de265_decoder_context) -> de265_error;

    pub fn de265_push_NAL(ctx: *mut de265_decoder_context, data: *const c_void, length: c_int, pts: de265_PTS, user_data: *mut c_void) -> de265_error;
    pub fn de265_flush_data(ctx: *mut de265_decoder_context) -> de265_error;
    pub fn de265_decode(ctx: *mut de265_decoder_context, more: *mut c_int) -> de265_error;

    pub fn de265_get_next_picture(ctx: *mut de265_decoder_context) -> *const de265_image;
    pub fn de265_peek_next_picture(ctx: *mut de265_decoder_context) -> *const de265_image;
    pub fn de265_release_next_picture(ctx: *mut de265_decoder_context);

    pub fn de265_get_image_width(img: *const de265_image, channel: c_int) -> c_int;
    pub fn de265_get_image_height(img: *const de265_image, channel: c_int) -> c_int;
    pub fn de265_get_chroma_format(img: *const de265_image) -> c_int;
    pub fn de265_get_bits_per_pixel(img: *const de265_image, channel: c_int) -> c_int;
    pub fn de265_get_image_plane(img: *const de265_image, channel: c_int, out_stride: *mut c_int) -> *const u8;
    pub fn de265_get_image_PTS(img: *const de265_image) -> de265_PTS;
}
