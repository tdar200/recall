//! Conversation audio encoding.
//!
//! A finalized conversation's PCM is encoded to a self-contained file buffer via
//! the [`AudioEncoder`] trait. The default [`WavEncoder`] is small, dependency-free
//! and fully verifiable here. The storage-optimizing **Opus** encoder (pure-Rust
//! `unsafe-libopus` + Ogg, ~7 MB/hr vs WAV's ~115 MB/hr) slots in behind the same
//! trait and should be validated against real on-device playback before it's relied
//! on for the "keep forever" archive.

use crate::error::{CoreError, Result};
use ogg::{PacketWriteEndInfo, PacketWriter};
use unsafe_libopus::{
    opus_encode, opus_encoder_create, opus_encoder_destroy, OpusEncoder as OpusEncoderState,
    OPUS_APPLICATION_VOIP, OPUS_OK,
};

/// Encodes i16 mono 16 kHz PCM into a self-contained audio file buffer.
pub trait AudioEncoder: Send {
    /// Encode a finalized conversation's PCM to file bytes.
    fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>>;
    /// File extension (without the dot), e.g. `"wav"`.
    fn extension(&self) -> &str;
}

/// Uncompressed 16-bit PCM WAV. Verified, playable, re-transcribable — the
/// working default until the Opus encoder is validated on-device.
pub struct WavEncoder {
    sample_rate: u32,
}

impl WavEncoder {
    pub fn new() -> Self {
        Self { sample_rate: 16_000 }
    }
}

impl Default for WavEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEncoder for WavEncoder {
    fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>> {
        Ok(write_wav(pcm, self.sample_rate))
    }
    fn extension(&self) -> &str {
        "wav"
    }
}

/// Serialize mono 16-bit PCM as a canonical 44-byte-header WAV file.
fn write_wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS: u16 = 16;
    let block_align = CHANNELS * (BITS / 8);
    let byte_rate = sample_rate * block_align as u32;
    let data_len = (pcm.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Ogg/Opus (`.opus`) — ~7 MB/hr vs WAV's ~115 MB/hr. Verified here by
/// round-trip decode; **validate against real playback on-device** before relying
/// on it for the permanent archive.
pub struct OpusEncoder {
    sample_rate: i32,
}

impl OpusEncoder {
    pub fn new() -> Self {
        Self { sample_rate: 16_000 }
    }
}

impl Default for OpusEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEncoder for OpusEncoder {
    fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>> {
        encode_opus_ogg(pcm, self.sample_rate)
    }
    fn extension(&self) -> &str {
        "opus"
    }
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::Io(e.to_string())
}

/// Build the RFC 7845 OpusHead identification header (channel mapping family 0).
fn opus_head(channels: u8, input_rate: u32, pre_skip: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1); // version
    v.push(channels);
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&input_rate.to_le_bytes());
    v.extend_from_slice(&0i16.to_le_bytes()); // output gain
    v.push(0); // channel mapping family 0
    v
}

/// Build the OpusTags comment header (vendor string, zero comments).
fn opus_tags(vendor: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"OpusTags");
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor.as_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // comment count
    v
}

fn encode_opus_ogg(pcm: &[i16], sample_rate: i32) -> Result<Vec<u8>> {
    let mut err = OPUS_OK;
    // SAFETY: standard libopus create/encode/destroy; pointers are valid for the call.
    let enc = unsafe { opus_encoder_create(sample_rate, 1, OPUS_APPLICATION_VOIP, &mut err) };
    if enc.is_null() || err != OPUS_OK {
        return Err(CoreError::Io(format!("opus encoder init failed ({err})")));
    }
    let out = encode_frames(enc, pcm, sample_rate);
    unsafe { opus_encoder_destroy(enc) };
    out
}

fn encode_frames(enc: *mut OpusEncoderState, pcm: &[i16], sample_rate: i32) -> Result<Vec<u8>> {
    const FRAME: usize = 320; // 20 ms @ 16 kHz (a valid Opus frame size)
    const SERIAL: u32 = 0x5245_4341; // "RECA"

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = PacketWriter::new(&mut buf);
        // Each header on its own page, per RFC 7845.
        w.write_packet(opus_head(1, sample_rate as u32, 0), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(io_err)?;
        w.write_packet(opus_tags("recall"), SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(io_err)?;

        let total = pcm.len().div_ceil(FRAME).max(1);
        let mut granule: u64 = 0;
        let mut frame = vec![0i16; FRAME];
        for fi in 0..total {
            let start = fi * FRAME;
            let end = (start + FRAME).min(pcm.len());
            frame[..end - start].copy_from_slice(&pcm[start..end]);
            for s in frame[end - start..].iter_mut() {
                *s = 0; // zero-pad the final partial frame
            }

            let mut packet = [0u8; 4000];
            // SAFETY: `enc` is valid; `frame` holds exactly FRAME samples; `packet` is writable.
            let n = unsafe {
                opus_encode(enc, frame.as_ptr(), FRAME as i32, packet.as_mut_ptr(), packet.len() as i32)
            };
            if n < 0 {
                return Err(CoreError::Io(format!("opus_encode failed ({n})")));
            }
            // Opus granule positions are always at 48 kHz.
            granule += FRAME as u64 * 48_000 / sample_rate as u64;
            let info = if fi + 1 == total {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            w.write_packet(packet[..n as usize].to_vec(), SERIAL, info, granule)
                .map_err(io_err)?;
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_encoder_writes_parseable_riff_header_and_all_samples() {
        let enc = WavEncoder::new();
        let pcm = vec![0i16, 100, -100, 32767, -32768];
        let bytes = enc.encode(&pcm).unwrap();

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");
        // 44-byte header + 2 bytes/sample, exactly.
        assert_eq!(bytes.len(), 44 + pcm.len() * 2);
        // data chunk size field matches the sample bytes.
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_len as usize, pcm.len() * 2);
        // first sample round-trips little-endian.
        assert_eq!(i16::from_le_bytes(bytes[44..46].try_into().unwrap()), 0);
        assert_eq!(i16::from_le_bytes(bytes[46..48].try_into().unwrap()), 100);

        assert_eq!(enc.extension(), "wav");
    }

    fn rms_i16(pcm: &[i16]) -> f64 {
        if pcm.is_empty() {
            return 0.0;
        }
        let s: f64 = pcm.iter().map(|&x| (x as f64) * (x as f64)).sum();
        (s / pcm.len() as f64).sqrt()
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// Decode an Ogg/Opus buffer back to PCM (test-only verification path).
    fn decode_opus_ogg(bytes: &[u8]) -> Vec<i16> {
        use unsafe_libopus::{opus_decode, opus_decoder_create, opus_decoder_destroy};
        let mut err = 0i32;
        let dec = unsafe { opus_decoder_create(16_000, 1, &mut err) };
        assert!(!dec.is_null());
        let mut out = Vec::new();
        let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
        while let Ok(Some(p)) = reader.read_packet() {
            if p.data.starts_with(b"OpusHead") || p.data.starts_with(b"OpusTags") {
                continue;
            }
            let mut pcm = [0i16; 5760];
            let n = unsafe {
                opus_decode(
                    dec,
                    p.data.as_ptr(),
                    p.data.len() as i32,
                    pcm.as_mut_ptr(),
                    pcm.len() as i32,
                    0,
                )
            };
            if n > 0 {
                out.extend_from_slice(&pcm[..n as usize]);
            }
        }
        unsafe { opus_decoder_destroy(dec) };
        out
    }

    #[test]
    fn opus_encoder_produces_decodable_ogg_opus_preserving_energy() {
        let enc = OpusEncoder::new();
        // 200 ms 300 Hz tone @ 16 kHz.
        let n = 16_000 * 200 / 1000;
        let pcm: Vec<i16> = (0..n)
            .map(|i| {
                let t = i as f64 / 16_000.0;
                (8000.0 * (2.0 * std::f64::consts::PI * 300.0 * t).sin()) as i16
            })
            .collect();

        let bytes = enc.encode(&pcm).unwrap();
        assert_eq!(&bytes[0..4], b"OggS", "must be an Ogg stream");
        assert!(contains(&bytes, b"OpusHead"));
        assert!(contains(&bytes, b"OpusTags"));
        assert_eq!(enc.extension(), "opus");
        // Opus must be far smaller than raw 16-bit PCM (fewer bytes than samples).
        assert!(
            bytes.len() < pcm.len(),
            "expected compression: {} bytes for {} samples",
            bytes.len(),
            pcm.len()
        );

        // Round-trip: decode back and confirm the signal survives (lossy → approximate).
        let decoded = decode_opus_ogg(&bytes);
        assert!(
            decoded.len() >= pcm.len() * 4 / 5,
            "decoded {} samples vs input {}",
            decoded.len(),
            pcm.len()
        );
        let (rms_in, rms_out) = (rms_i16(&pcm), rms_i16(&decoded));
        assert!(
            (rms_out - rms_in).abs() / rms_in < 0.4,
            "energy mismatch: in={rms_in:.0} out={rms_out:.0}"
        );
    }
}
