//! Video playback (`FunkinVideoSprite` / `FlxVideo`). Everything is portable
//! and built into the game, so players only need the base game's files:
//!
//! - containers: MP4 (`mp4`) and Matroska (`matroska-demuxer`);
//! - video: H.264 (OpenH264, built from source), H.265 (libde265, built from
//!   source by `de265-sys`) and VP9 (`rusty_vp9`, pure Rust);
//! - audio: AAC (Symphonia) and Vorbis (lewton).
//!
//! The file is demuxed up front; the video decodes on a background thread a
//! few frames ahead and each shown frame replaces one texture.

use std::io::Cursor;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;

use openh264::decoder::{Decoder, DecoderConfig, Flush};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

use crate::audio::{Audio, Pcm};
use crate::gfx::{Gfx, TexId, VIEW_H, VIEW_W, WHITE};

struct Frame {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

pub struct Video {
    rx: Receiver<Frame>,
    fps: f64,
    duration: f64,
    audio: Option<Arc<Pcm>>,
    started: bool,
    time: f64,
    /// Frames taken from the decoder so far.
    shown: u64,
    tex: Option<TexId>,
    decoder_done: bool,
    /// The soundtrack once playing (stopped when the video is dropped), and its volume.
    sound: Option<(crate::audio::SfxHandle, u64)>,
    volume: f32,
    paused: bool,
}

impl Drop for Video {
    fn drop(&mut self) {
        // A restarted song, a skipped cutscene: the soundtrack goes with the picture.
        if let Some((h, id)) = self.sound.take() {
            h.stop(id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Codec {
    H264,
    Hevc,
    Vp9,
}

/// A demuxed video track.
struct Track {
    codec: Codec,
    /// Parameter sets (H.264 SPS/PPS, H.265 VPS/SPS/PPS), without start codes.
    params: Vec<Vec<u8>>,
    /// Bytes in each NAL length prefix (H.264 / H.265).
    nal_len: usize,
    /// Coded frames in decode order.
    samples: Vec<Vec<u8>>,
    fps: f64,
    duration: f64,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("video: {e}")
}

// ---- containers ------------------------------------------------------------

/// Read `count` length-prefixed parameter sets (`u16` size each).
fn read_sets(b: &[u8], mut i: usize, count: usize, out: &mut Vec<Vec<u8>>) -> Option<usize> {
    for _ in 0..count {
        let n = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        i += 2;
        out.push(b.get(i..i + n)?.to_vec());
        i += n;
    }
    Some(i)
}

/// `AVCDecoderConfigurationRecord` → (NAL length size, SPS + PPS).
fn parse_avcc(b: &[u8]) -> Option<(usize, Vec<Vec<u8>>)> {
    let nal_len = (*b.get(4)? & 3) as usize + 1;
    let mut sets = Vec::new();
    let i = read_sets(b, 6, (*b.get(5)? & 31) as usize, &mut sets)?;
    read_sets(b, i + 1, *b.get(i)? as usize, &mut sets)?;
    Some((nal_len, sets))
}

/// `HEVCDecoderConfigurationRecord` → (NAL length size, VPS + SPS + PPS + SEI).
fn parse_hvcc(b: &[u8]) -> Option<(usize, Vec<Vec<u8>>)> {
    let nal_len = (*b.get(21)? & 3) as usize + 1;
    let arrays = *b.get(22)? as usize;
    let mut sets = Vec::new();
    let mut i = 23;
    for _ in 0..arrays {
        let n = u16::from_be_bytes([*b.get(i + 1)?, *b.get(i + 2)?]) as usize;
        i = read_sets(b, i + 3, n, &mut sets)?;
    }
    Some((nal_len, sets))
}

/// The payload of the first `kind` box inside the top-level `moov` box
/// (the `mp4` crate reads `hev1` but not `hvc1`, so `hvcC` is found here).
fn find_in_moov<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut i = 0usize;
    while i + 8 <= b.len() {
        let mut size = u32::from_be_bytes(b[i..i + 4].try_into().ok()?) as u64;
        let mut head = 8;
        if size == 1 {
            size = u64::from_be_bytes(b.get(i + 8..i + 16)?.try_into().ok()?);
            head = 16;
        } else if size == 0 {
            size = (b.len() - i) as u64;
        }
        let end = (i as u64 + size).min(b.len() as u64) as usize;
        if &b[i + 4..i + 8] == b"moov" {
            let moov = &b[i + head..end];
            let at = moov.windows(4).position(|w| w == kind)?;
            if at < 4 {
                return None;
            }
            let n = u32::from_be_bytes(moov[at - 4..at].try_into().ok()?) as usize;
            return moov.get(at + 4..at - 4 + n);
        }
        if size < 8 {
            return None;
        }
        i = end;
    }
    None
}

fn demux_mp4(bytes: &[u8]) -> Result<(Track, Option<Pcm>), String> {
    let size = bytes.len() as u64;
    let mut reader = mp4::Mp4Reader::read_header(Cursor::new(bytes), size).map_err(err)?;
    let mut video_id = None;
    let mut audio_id = None;
    for (id, t) in reader.tracks() {
        match t.track_type() {
            Ok(mp4::TrackType::Video) => video_id = video_id.or(Some(*id)),
            Ok(mp4::TrackType::Audio) => audio_id = audio_id.or(Some(*id)),
            _ => {}
        }
    }
    let video_id = video_id.ok_or("video: no video track")?;
    let track = &reader.tracks()[&video_id];
    let (codec, nal_len, params) = match track.media_type() {
        Ok(mp4::MediaType::H264) => {
            let sets = vec![track.sequence_parameter_set().map_err(err)?.to_vec(), track.picture_parameter_set().map_err(err)?.to_vec()];
            let nal_len = find_in_moov(bytes, b"avcC").and_then(parse_avcc).map(|(n, _)| n).unwrap_or(4);
            (Codec::H264, nal_len, sets)
        }
        Ok(mp4::MediaType::VP9) => (Codec::Vp9, 0, Vec::new()),
        _ => match find_in_moov(bytes, b"hvcC").and_then(parse_hvcc) {
            Some((n, sets)) => (Codec::Hevc, n, sets),
            None => return Err("video: unsupported video codec".into()),
        },
    };
    let count = track.sample_count();
    let duration = track.duration().as_secs_f64();
    let fps = if duration > 0.0 { count as f64 / duration } else { 24.0 };
    let mut samples = Vec::with_capacity(count as usize);
    for s in 1..=count {
        match reader.read_sample(video_id, s).map_err(err)? {
            Some(sample) => samples.push(sample.bytes.to_vec()),
            None => break,
        }
    }
    let audio = audio_id.and_then(|id| decode_aac_mp4(&mut reader, id).map_err(|e| log::warn!("{e}")).ok());
    Ok((Track { codec, params, nal_len, samples, fps, duration }, audio))
}

fn demux_mkv(bytes: &[u8]) -> Result<(Track, Option<Pcm>), String> {
    use matroska_demuxer::{Frame as MkvFrame, MatroskaFile, TrackType};
    let mut mkv = MatroskaFile::open(Cursor::new(bytes)).map_err(err)?;
    let scale = mkv.info().timestamp_scale().get() as f64; // ns per tick
    let file_duration = mkv.info().duration().map(|d| d * scale / 1e9);
    let tracks = mkv.tracks().to_vec();
    let video = tracks.iter().find(|t| t.track_type() == TrackType::Video).ok_or("video: no video track")?;
    let private = video.codec_private().unwrap_or_default();
    let (codec, nal_len, params) = match video.codec_id() {
        "V_MPEG4/ISO/AVC" => {
            let (n, sets) = parse_avcc(private).ok_or("video: bad avcC")?;
            (Codec::H264, n, sets)
        }
        "V_MPEGH/ISO/HEVC" => {
            let (n, sets) = parse_hvcc(private).ok_or("video: bad hvcC")?;
            (Codec::Hevc, n, sets)
        }
        "V_VP9" => (Codec::Vp9, 0, Vec::new()),
        other => return Err(format!("video: unsupported video codec {other}")),
    };
    let video_no = video.track_number().get();
    let frame_ns = video.default_duration().map(|d| d.get() as f64);
    let audio_track = tracks.iter().find(|t| t.track_type() == TrackType::Audio);
    let audio_no = audio_track.map(|t| t.track_number().get());

    let mut samples = Vec::new();
    let mut packets = Vec::new();
    let mut last_ts = 0u64;
    let mut frame = MkvFrame::default();
    while mkv.next_frame(&mut frame).map_err(err)? {
        if frame.track == video_no {
            last_ts = last_ts.max(frame.timestamp);
            samples.push(std::mem::take(&mut frame.data));
        } else if Some(frame.track) == audio_no {
            packets.push(std::mem::take(&mut frame.data));
        }
    }
    let fps = match frame_ns {
        Some(ns) if ns > 0.0 => 1e9 / ns,
        _ if last_ts > 0 && samples.len() > 1 => (samples.len() - 1) as f64 / (last_ts as f64 * scale / 1e9),
        _ => 24.0,
    };
    let duration = file_duration.unwrap_or(samples.len() as f64 / fps);
    let audio = match audio_track {
        Some(t) if t.codec_id() == "A_VORBIS" => decode_vorbis(t.codec_private().unwrap_or_default(), &packets).map_err(|e| log::warn!("{e}")).ok(),
        Some(t) => {
            log::warn!("video: unsupported audio codec {}", t.codec_id());
            None
        }
        None => None,
    };
    Ok((Track { codec, params, nal_len, samples, fps, duration }, audio))
}

// ---- audio -------------------------------------------------------------------

fn push_stereo(out: &mut Vec<f32>, interleaved: &[f32], channels: usize) {
    match channels {
        0 => {}
        1 => out.extend(interleaved.iter().flat_map(|&v| [v, v])),
        2 => out.extend_from_slice(interleaved),
        n => out.extend(interleaved.chunks(n).flat_map(|c| [c[0], c[1]])),
    }
}

fn decode_aac_mp4(reader: &mut mp4::Mp4Reader<Cursor<&[u8]>>, id: u32) -> Result<Pcm, String> {
    use symphonia::core::audio::{Channels, SampleBuffer};
    use symphonia::core::codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_AAC};
    use symphonia::core::formats::Packet;

    let track = reader.tracks().get(&id).ok_or("video: no audio track")?;
    let profile = track.audio_profile().map_err(err)? as u8;
    let freq = track.sample_freq_index().map_err(err)?;
    let chans = track.channel_config().map_err(err)? as u8;
    let rate = freq.freq();
    let freq = freq as u8;
    let count = track.sample_count();
    // AudioSpecificConfig: 5 bits object type, 4 bits frequency index, 4 bits channels.
    let asc = (((profile as u16) << 11) | ((freq as u16) << 7) | ((chans as u16) << 3)).to_be_bytes();
    let channels = if chans == 1 { Channels::FRONT_LEFT } else { Channels::FRONT_LEFT | Channels::FRONT_RIGHT };
    let mut params = CodecParameters::new();
    params.for_codec(CODEC_TYPE_AAC).with_sample_rate(rate).with_channels(channels).with_extra_data(Box::new(asc));
    let mut dec = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(err)?;

    let mut samples = Vec::new();
    for s in 1..=count {
        let Some(sample) = reader.read_sample(id, s).map_err(err)? else { break };
        let pkt = Packet::new_from_slice(0, sample.start_time, sample.duration as u64, &sample.bytes);
        let buf = match dec.decode(&pkt) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("video: AAC packet {s}: {e}");
                continue;
            }
        };
        let spec = *buf.spec();
        let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(buf);
        push_stereo(&mut samples, sb.samples(), spec.channels.count());
    }
    Ok(Pcm { samples, rate })
}

/// Matroska `A_VORBIS`: the private data holds the three headers, Xiph-laced.
fn decode_vorbis(private: &[u8], packets: &[Vec<u8>]) -> Result<Pcm, String> {
    use lewton::audio::{read_audio_packet_generic, PreviousWindowRight};
    use lewton::header::{read_header_comment, read_header_ident, read_header_setup};
    use lewton::samples::InterleavedSamples;

    let count = *private.first().ok_or("video: empty Vorbis headers")? as usize + 1;
    if count != 3 {
        return Err("video: expected three Vorbis headers".into());
    }
    let mut i = 1;
    let mut sizes = Vec::new();
    for _ in 0..count - 1 {
        let mut n = 0usize;
        loop {
            let b = *private.get(i).ok_or("video: bad Vorbis headers")?;
            i += 1;
            n += b as usize;
            if b != 255 {
                break;
            }
        }
        sizes.push(n);
    }
    let ident_end = i + sizes[0];
    let comment_end = ident_end + sizes[1];
    let get = |a: usize, b: usize| private.get(a..b).ok_or_else(|| "video: bad Vorbis headers".to_owned());
    let ident = read_header_ident(get(i, ident_end)?).map_err(err)?;
    read_header_comment(get(ident_end, comment_end)?).map_err(err)?;
    let setup = read_header_setup(get(comment_end, private.len())?, ident.audio_channels, (ident.blocksize_0, ident.blocksize_1)).map_err(err)?;

    let mut pwr = PreviousWindowRight::new();
    let mut samples = Vec::new();
    for p in packets {
        match read_audio_packet_generic::<InterleavedSamples<f32>>(&ident, &setup, p, &mut pwr) {
            Ok(s) => push_stereo(&mut samples, &s.samples, s.channel_count),
            Err(e) => log::debug!("video: Vorbis packet: {e}"),
        }
    }
    Ok(Pcm { samples, rate: ident.audio_sample_rate })
}

// ---- video decoders ----------------------------------------------------------

/// Limited-range BT.709 YUV → RGBA. `px(plane, x, y)` reads one 8-bit sample.
fn yuv_to_rgba(w: usize, h: usize, (sx, sy): (usize, usize), px: impl Fn(usize, usize, usize) -> u8) -> Vec<u8> {
    let mut rgba = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let yy = (px(0, x, y) as f32 - 16.0) * 1.164;
            let u = px(1, x >> sx, y >> sy) as f32 - 128.0;
            let v = px(2, x >> sx, y >> sy) as f32 - 128.0;
            let o = (y * w + x) * 4;
            rgba[o] = (yy + 1.793 * v).clamp(0.0, 255.0) as u8;
            rgba[o + 1] = (yy - 0.213 * u - 0.533 * v).clamp(0.0, 255.0) as u8;
            rgba[o + 2] = (yy + 2.112 * u).clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

/// Split length-prefixed NAL units.
fn nals(sample: &[u8], len: usize) -> impl Iterator<Item = &[u8]> {
    let mut i = 0;
    std::iter::from_fn(move || {
        if len == 0 || i + len > sample.len() {
            return None;
        }
        let n = sample[i..i + len].iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        i += len;
        let end = (i + n).min(sample.len());
        let nal = &sample[i..end];
        i = end;
        Some(nal)
    })
}

/// The decode thread: returns once every frame is sent or the player is gone.
fn decode(track: Track, tx: SyncSender<Frame>) {
    let result = match track.codec {
        Codec::H264 => decode_h264(&track, &tx),
        Codec::Hevc => decode_hevc(&track, &tx),
        Codec::Vp9 => decode_vp9(&track, &tx),
    };
    if let Err(e) = result {
        log::warn!("{e}");
    }
}

/// `Err` only for a decoder that can't start; a closed channel just stops.
type DecodeResult = Result<(), String>;

fn decode_h264(track: &Track, tx: &SyncSender<Frame>) -> DecodeResult {
    // Flushing after every packet breaks B-frame reordering; flush once at the end.
    let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
    let mut dec = Decoder::with_api_config(OpenH264API::from_source(), config).map_err(err)?;
    let to_frame = |yuv: &openh264::decoder::DecodedYUV<'_>| {
        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Frame { w: w as u32, h: h as u32, rgba }
    };
    let mut packet = Vec::new();
    for ps in &track.params {
        packet.extend_from_slice(&[0, 0, 0, 1]);
        packet.extend_from_slice(ps);
    }
    for (s, sample) in track.samples.iter().enumerate() {
        for nal in nals(sample, track.nal_len) {
            packet.extend_from_slice(&[0, 0, 0, 1]);
            packet.extend_from_slice(nal);
        }
        match dec.decode(&packet) {
            Ok(Some(yuv)) => {
                if tx.send(to_frame(&yuv)).is_err() {
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(e) => log::debug!("video: frame {s}: {e}"),
        }
        packet.clear();
    }
    if let Ok(rest) = dec.flush_remaining() {
        for yuv in &rest {
            if tx.send(to_frame(yuv)).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn decode_hevc(track: &Track, tx: &SyncSender<Frame>) -> DecodeResult {
    use de265_sys::*;

    struct Ctx(*mut de265_decoder_context);
    impl Drop for Ctx {
        fn drop(&mut self) {
            unsafe { de265_free_decoder(self.0) };
        }
    }

    /// Send every finished picture; `false` once the player is gone.
    fn drain(ctx: *mut de265_decoder_context, tx: &SyncSender<Frame>) -> bool {
        loop {
            let img = unsafe { de265_get_next_picture(ctx) };
            if img.is_null() {
                return true;
            }
            let frame = unsafe { picture(img) };
            unsafe { de265_release_next_picture(ctx) };
            if let Some(f) = frame {
                if tx.send(f).is_err() {
                    return false;
                }
            }
        }
    }

    unsafe fn picture(img: *const de265_image) -> Option<Frame> {
        let w = de265_get_image_width(img, 0).max(0) as usize;
        let h = de265_get_image_height(img, 0).max(0) as usize;
        let (sx, sy) = match de265_get_chroma_format(img) {
            DE265_CHROMA_420 => (1, 1),
            DE265_CHROMA_422 => (1, 0),
            DE265_CHROMA_444 => (0, 0),
            _ => return None,
        };
        let mut planes = [(std::ptr::null(), 0usize, 0u32); 3];
        for (c, p) in planes.iter_mut().enumerate() {
            let mut stride = 0;
            let ptr = de265_get_image_plane(img, c as i32, &mut stride);
            if ptr.is_null() {
                return None;
            }
            *p = (ptr, stride.max(0) as usize, de265_get_bits_per_pixel(img, c as i32).max(8) as u32);
        }
        let rgba = yuv_to_rgba(w, h, (sx, sy), |c, x, y| {
            let (ptr, stride, bits) = planes[c];
            if bits > 8 {
                let at = ptr.add(y * stride + x * 2) as *const u16;
                (at.read_unaligned() >> (bits - 8)) as u8
            } else {
                *ptr.add(y * stride + x)
            }
        });
        Some(Frame { w: w as u32, h: h as u32, rgba })
    }

    let ctx = Ctx(unsafe { de265_new_decoder() });
    if ctx.0.is_null() {
        return Err("video: libde265 failed to start".into());
    }
    // One thread already decodes the 1080p cutscenes faster than real time,
    // so libde265's worker pool stays off.
    for ps in &track.params {
        unsafe { de265_push_NAL(ctx.0, ps.as_ptr().cast(), ps.len() as i32, 0, std::ptr::null_mut()) };
    }
    let run = |ctx: *mut de265_decoder_context| -> bool {
        loop {
            let mut more = 0;
            let e = unsafe { de265_decode(ctx, &mut more) };
            if !drain(ctx, tx) {
                return false;
            }
            if more == 0 || e == DE265_ERROR_WAITING_FOR_INPUT_DATA {
                return true;
            }
            if e != DE265_OK && e != DE265_ERROR_IMAGE_BUFFER_FULL && unsafe { de265_isOK(e) } == 0 {
                log::debug!("video: libde265 error {e}");
            }
        }
    };
    for (s, sample) in track.samples.iter().enumerate() {
        for nal in nals(sample, track.nal_len) {
            unsafe { de265_push_NAL(ctx.0, nal.as_ptr().cast(), nal.len() as i32, s as i64, std::ptr::null_mut()) };
        }
        if !run(ctx.0) {
            return Ok(());
        }
    }
    unsafe { de265_flush_data(ctx.0) };
    run(ctx.0);
    Ok(())
}

fn decode_vp9(track: &Track, tx: &SyncSender<Frame>) -> DecodeResult {
    use rusty_vp9::{Error, Vp9Decoder};

    fn picture(f: &rusty_vp9::DecodedFrame) -> Option<Frame> {
        let (w, h) = (f.width as usize, f.height as usize);
        if f.planes.len() < 3 || f.strides.len() < 3 {
            return None;
        }
        let shift = f.bit_depth.saturating_sub(8);
        let rgba = yuv_to_rgba(w, h, (f.subsampling_x as usize, f.subsampling_y as usize), |c, x, y| {
            let (p, stride) = (&f.planes[c], f.strides[c]);
            if shift > 0 {
                let at = y * stride + x * 2;
                (u16::from_le_bytes([p[at], p[at + 1]]) >> shift) as u8
            } else {
                p[y * stride + x]
            }
        });
        Some(Frame { w: w as u32, h: h as u32, rgba })
    }

    // `false` once the player is gone.
    let drain = |dec: &mut Vp9Decoder| -> bool {
        loop {
            match dec.next_frame() {
                Ok(f) => {
                    if let Some(frame) = picture(&f) {
                        if tx.send(frame).is_err() {
                            return false;
                        }
                    }
                }
                Err(Error::Again) | Err(Error::Eof) => return true,
                Err(e) => {
                    log::debug!("video: VP9: {e:?}");
                    return true;
                }
            }
        }
    };
    let mut dec = Vp9Decoder::new();
    for (s, sample) in track.samples.iter().enumerate() {
        if let Err(e) = dec.push(sample, Some(s as i64)) {
            log::debug!("video: VP9 frame {s}: {e:?}");
        }
        if !drain(&mut dec) {
            return Ok(());
        }
    }
    dec.flush();
    drain(&mut dec);
    Ok(())
}

impl Video {
    /// Open an MP4 or Matroska file from memory and start decoding.
    pub fn open(bytes: Vec<u8>) -> Result<Self, String> {
        let (track, audio) = if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) { demux_mkv(&bytes)? } else { demux_mp4(&bytes)? };
        drop(bytes);
        log::info!("video: {:?}, {} frames at {:.2} fps, {:.2}s", track.codec, track.samples.len(), track.fps, track.duration);
        let (fps, duration) = (track.fps, track.duration);
        let (tx, rx) = sync_channel::<Frame>(4);
        std::thread::Builder::new().name("video".into()).spawn(move || decode(track, tx)).map_err(err)?;
        Ok(Self { rx, fps, duration, audio: audio.map(Arc::new), started: false, time: 0.0, shown: 0, tex: None, decoder_done: false, sound: None, volume: 1.0, paused: false })
    }

    /// Advance; returns `true` once the video has ended.
    pub fn update(&mut self, dt: f32, gfx: &mut Gfx, audio: &Audio) -> bool {
        if !self.started {
            // Wait for the first frame so the picture and sound start together.
            match self.rx.try_recv() {
                Ok(f) => self.show(gfx, f),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
            self.started = true;
            if let Some(a) = &self.audio {
                let id = audio.play_sfx_tracked(a.clone(), self.volume);
                if self.paused {
                    audio.set_sfx_paused(id, true);
                }
                self.sound = Some((audio.sfx_handle(), id));
            }
            return false;
        }
        self.time += dt as f64;
        let want = (self.time * self.fps) as u64 + 1;
        while self.shown < want {
            match self.rx.try_recv() {
                Ok(f) => self.show(gfx, f),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.decoder_done = true;
                    break;
                }
            }
        }
        self.decoder_done && self.time >= self.duration.min(self.shown as f64 / self.fps)
    }

    /// The current picture (the same texture all along, refilled per frame).
    pub fn texture(&self) -> Option<TexId> {
        self.tex
    }

    /// Playback position in seconds.
    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn set_volume(&mut self, audio: &Audio, v: f32) {
        self.volume = v;
        if let Some((_, id)) = &self.sound {
            audio.set_sfx_volume(*id, v);
        }
    }

    /// Jump ahead to `secs` (hxvlc's `time = x`, for resyncing). The decoder only runs
    /// forwards, so an earlier time is ignored; frames up to the new time are skipped.
    pub fn seek(&mut self, audio: &Audio, secs: f64) {
        if secs <= self.time + 0.05 || !self.started {
            return;
        }
        self.time = secs;
        if let Some((h, id)) = self.sound.take() {
            h.stop(id);
            if let Some(a) = &self.audio {
                if secs < self.duration {
                    let id = audio.play_sfx_from(a.clone(), self.volume, secs);
                    if self.paused {
                        audio.set_sfx_paused(id, true);
                    }
                    self.sound = Some((audio.sfx_handle(), id));
                }
            }
        }
    }

    /// Hold the soundtrack (the picture holds while `update` isn't called).
    pub fn set_paused(&mut self, audio: &Audio, paused: bool) {
        self.paused = paused;
        if let Some((_, id)) = &self.sound {
            audio.set_sfx_paused(*id, paused);
        }
    }

    /// Stop the soundtrack (the video is being dropped).
    pub fn stop(&mut self, _audio: &Audio) {
        if let Some((h, id)) = self.sound.take() {
            h.stop(id);
        }
    }

    fn show(&mut self, gfx: &mut Gfx, f: Frame) {
        self.shown += 1;
        match self.tex {
            Some(t) => gfx.replace_rgba(t, f.w, f.h, &f.rgba),
            None => self.tex = Some(gfx.upload_rgba(f.w, f.h, &f.rgba)),
        }
    }

    /// Draw fitted to the game's 1280×720 view and centred (`VideoCutscene`).
    pub fn draw(&self, gfx: &mut Gfx) {
        if let Some(t) = self.tex {
            let (w, h) = gfx.texture_size(t);
            let k = (VIEW_W / w).min(VIEW_H / h);
            let (dw, dh) = (w * k, h * k);
            gfx.image(t, [0.0, 0.0, w, h], (VIEW_W - dw) / 2.0, (VIEW_H - dh) / 2.0, dw, dh, WHITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs real assets: `RUSTED_MIC_TEST_ASSETS=<assets folder> cargo test -- --ignored`.
    /// Set `RUSTED_MIC_VIDEO_DUMP=<folder>` to save some frames as PNG.
    fn decodes(file: &str) {
        let Some(root) = std::env::var_os("RUSTED_MIC_TEST_ASSETS") else { return };
        let path = std::path::PathBuf::from(root).join("videos").join("videos").join(file);
        let start = std::time::Instant::now();
        let v = Video::open(std::fs::read(path).unwrap()).unwrap();
        let audio = v.audio.as_ref().expect("audio track");
        let secs = audio.frames() as f64 / audio.rate as f64;
        assert!((secs - v.duration).abs() < 0.5, "{file}: audio {secs}s vs video {}s", v.duration);
        let dump = std::env::var_os("RUSTED_MIC_VIDEO_DUMP").map(std::path::PathBuf::from);
        let stem = file.split('.').next().unwrap();
        let mut n = 0;
        while let Ok(f) = v.rx.recv() {
            if let Some(d) = &dump {
                if n % 48 == 0 {
                    image::save_buffer(d.join(format!("{stem}-{n:04}.png")), &f.rgba, f.w, f.h, image::ExtendedColorType::Rgba8).unwrap();
                }
            }
            n += 1;
        }
        let took = start.elapsed().as_secs_f64();
        println!("{file}: {n} frames at {:.2} fps, {:.2}s (decoded in {took:.2}s), audio {secs:.2}s @ {} Hz", v.fps, v.duration, audio.rate);
        assert!(n as f64 >= v.duration * v.fps - 5.0, "{file}: only {n} frames");
    }

    #[test]
    #[ignore]
    fn decodes_h264_mp4() {
        decodes("introSelect.mp4");
    }

    #[test]
    #[ignore]
    fn decodes_h264_mkv() {
        decodes("ughCutscene.mkv");
    }

    #[test]
    #[ignore]
    fn decodes_hevc_mp4() {
        decodes("2hotCutscene.mp4");
    }

    #[test]
    #[ignore]
    fn decodes_vp9_mp4() {
        decodes("darnellCutscene.mp4");
    }
}

