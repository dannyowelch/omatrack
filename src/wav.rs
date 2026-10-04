//! WAV files.
//!
//! Song renders write 16-bit stereo PCM. Sample import reads the common
//! encodings (8/16/24-bit PCM, 32-bit float, including `WAVEFORMATEXTENSIBLE`)
//! into interleaved `f32`. Sample export writes 8-bit unsigned mono, which is
//! how a `.wav` stores the signed bytes a `.mod` keeps.
//!
//! Conversion of that PCM into a ProTracker sample lives in [`crate::convert`].

use std::fs;
use std::path::Path;

use crate::error::{Error, IoKind};

/// PCM decoded from a WAV file, before it is turned into a module sample.
///
/// Samples are interleaved and scaled to about `-1.0..=1.0`. 8-bit WAV is
/// unsigned in the file; it is centered here. Non-finite floats become silence.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels stored in [`Self::interleaved`].
    pub channels: u16,
    /// Interleaved frames. Length is a multiple of `channels`.
    pub interleaved: Vec<f32>,
}

/// Encode interleaved stereo `i16` samples as a WAV byte string.
pub fn wav_bytes(sample_rate: u32, interleaved_stereo: &[i16]) -> Result<Vec<u8>, Error> {
    if sample_rate == 0 {
        return Err(Error::Audio(
            "sample rate must be greater than zero to write a WAV file".to_string(),
        ));
    }
    let data_bytes = interleaved_stereo
        .len()
        .checked_mul(2)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or_else(|| Error::Audio("rendered audio is too long for a WAV file".to_string()))?;
    let chunk_size = 36u32.saturating_add(data_bytes);
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate.saturating_mul(4);
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in interleaved_stereo {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(out)
}

/// Write interleaved stereo `i16` samples to `path`.
pub fn write_wav(path: &Path, sample_rate: u32, interleaved_stereo: &[i16]) -> Result<(), Error> {
    let bytes = wav_bytes(sample_rate, interleaved_stereo)?;
    fs::write(path, bytes)
        .map_err(|source| Error::io(IoKind::Write, path.to_path_buf(), source))?;
    Ok(())
}

/// Encode signed 8-bit mono PCM as an 8-bit unsigned WAV.
///
/// `signed_pcm` uses the same byte convention as a `.mod` sample (`0x00` is 0,
/// `0xFF` is −1). The WAV file stores those bytes plus 128.
pub fn encode_mono8_wav(sample_rate: u32, signed_pcm: &[u8]) -> Result<Vec<u8>, Error> {
    if sample_rate == 0 {
        return Err(Error::Wav(
            "sample rate must be greater than zero to write a WAV file".to_string(),
        ));
    }
    let data_bytes = u32::try_from(signed_pcm.len())
        .map_err(|_| Error::Wav("sample is too long for a WAV file".to_string()))?;
    let chunk_size = 36u32.saturating_add(data_bytes);
    let mut out = Vec::with_capacity(44 + signed_pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&chunk_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes()); // byte rate
    out.extend_from_slice(&1u16.to_le_bytes()); // block align
    out.extend_from_slice(&8u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in signed_pcm {
        out.push(signed_to_wav8(*sample));
    }
    Ok(out)
}

/// Write a signed 8-bit mono sample to `path` as an 8-bit WAV.
pub fn write_mono8_wav(path: &Path, sample_rate: u32, signed_pcm: &[u8]) -> Result<(), Error> {
    let bytes = encode_mono8_wav(sample_rate, signed_pcm)?;
    fs::write(path, bytes)
        .map_err(|source| Error::io(IoKind::Write, path.to_path_buf(), source))?;
    Ok(())
}

/// Decode a WAV byte string into interleaved `f32` frames.
///
/// Accepts PCM at 8, 16, or 24 bits and 32-bit IEEE float, mono or multi-channel,
/// including `WAVEFORMATEXTENSIBLE` when the subformat is one of those.
/// Other encodings return [`Error::Wav`] naming what was found.
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, Error> {
    if bytes.len() < 12 {
        return Err(Error::Wav(
            "file is too short to be a WAV (need a RIFF header)".to_string(),
        ));
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Wav(
            "file is not a RIFF WAVE (missing RIFF/WAVE header)".to_string(),
        ));
    }

    let mut offset = 12usize;
    let mut fmt: Option<WavFormat> = None;
    let mut data: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(read_array(bytes, offset + 4)?) as usize;
        let data_at = offset + 8;
        let data_end = data_at
            .checked_add(size)
            .ok_or_else(|| Error::Wav("WAV chunk size overflows the file".to_string()))?;
        if data_end > bytes.len() {
            return Err(Error::Wav(format!(
                "WAV chunk is truncated: {} claims {size} bytes, file ends at {}",
                chunk_name(id),
                bytes.len()
            )));
        }
        let chunk = &bytes[data_at..data_end];
        match id {
            b"fmt " => fmt = Some(parse_fmt(chunk)?),
            b"data" => data = Some(chunk),
            _ => {}
        }
        let padded = size + (size % 2);
        offset = data_at
            .checked_add(padded)
            .ok_or_else(|| Error::Wav("WAV chunk size overflows the file".to_string()))?;
    }
    let fmt = fmt.ok_or_else(|| Error::Wav("WAV file has no fmt chunk".to_string()))?;
    let data = data.ok_or_else(|| Error::Wav("WAV file has no data chunk".to_string()))?;
    if fmt.sample_rate == 0 {
        return Err(Error::Wav("WAV sample rate is 0".to_string()));
    }
    if fmt.channels == 0 {
        return Err(Error::Wav("WAV has no channels".to_string()));
    }
    if fmt.channels > 64 {
        return Err(Error::Wav(format!(
            "WAV has {} channels; omatrack can downmix up to 64",
            fmt.channels
        )));
    }
    let interleaved = decode_frames(&fmt, data)?;
    Ok(DecodedWav {
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
        interleaved,
    })
}

/// Read a WAV from disk and decode it.
pub fn read_wav(path: &Path) -> Result<DecodedWav, Error> {
    let bytes =
        fs::read(path).map_err(|source| Error::io(IoKind::Read, path.to_path_buf(), source))?;
    decode_wav(&bytes).map_err(|err| match err {
        Error::Wav(message) => Error::Wav(format!("{}: {message}", path.display())),
        other => other,
    })
}

#[derive(Clone, Copy)]
struct WavFormat {
    format: SampleEncoding,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    block_align: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleEncoding {
    Pcm,
    Float,
}

fn parse_fmt(chunk: &[u8]) -> Result<WavFormat, Error> {
    if chunk.len() < 16 {
        return Err(Error::Wav(format!(
            "WAV fmt chunk is {} bytes; need at least 16",
            chunk.len()
        )));
    }
    let audio_format = u16::from_le_bytes([chunk[0], chunk[1]]);
    let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
    let sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
    let block_align = u16::from_le_bytes([chunk[12], chunk[13]]);
    let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
    let (format, bits) = match audio_format {
        1 => (SampleEncoding::Pcm, bits),
        3 => (SampleEncoding::Float, bits),
        0xFFFE => parse_extensible(chunk, bits)?,
        other => {
            return Err(Error::Wav(format!(
                "unsupported WAV format {other}; use 8, 16, or 24-bit PCM or 32-bit float"
            )))
        }
    };
    match format {
        SampleEncoding::Pcm if matches!(bits, 8 | 16 | 24) => {}
        SampleEncoding::Float if bits == 32 => {}
        SampleEncoding::Pcm => {
            return Err(Error::Wav(format!(
                "unsupported WAV: {bits}-bit PCM; use 8, 16, or 24-bit PCM or 32-bit float"
            )))
        }
        SampleEncoding::Float => {
            return Err(Error::Wav(format!(
                "unsupported WAV: {bits}-bit float; only 32-bit float is supported"
            )))
        }
    }
    Ok(WavFormat {
        format,
        channels,
        sample_rate,
        bits,
        block_align,
    })
}

fn parse_extensible(chunk: &[u8], bits: u16) -> Result<(SampleEncoding, u16), Error> {
    if chunk.len() < 40 {
        return Err(Error::Wav(
            "unsupported WAV: WAVEFORMATEXTENSIBLE fmt is shorter than 40 bytes".to_string(),
        ));
    }
    let sub = &chunk[24..40];
    let tag = u16::from_le_bytes([sub[0], sub[1]]);
    // The rest of the GUID is the standard WAVE suffix.
    const SUFFIX: [u8; 14] = [
        0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
    ];
    if sub[2..] != SUFFIX {
        return Err(Error::Wav(
            "unsupported WAV: extensible subformat is not PCM or IEEE float".to_string(),
        ));
    }
    let format = match tag {
        1 => SampleEncoding::Pcm,
        3 => SampleEncoding::Float,
        other => {
            return Err(Error::Wav(format!(
                "unsupported WAV extensible subformat {other}; use PCM or 32-bit float"
            )))
        }
    };
    Ok((format, bits))
}

fn decode_frames(fmt: &WavFormat, data: &[u8]) -> Result<Vec<f32>, Error> {
    let channels = usize::from(fmt.channels);
    let bps = usize::from(fmt.bits);
    let width = bps / 8;
    if width == 0 {
        return Err(Error::Wav("WAV bits per sample is 0".to_string()));
    }
    let stored = if fmt.block_align == 0 {
        width * channels
    } else {
        usize::from(fmt.block_align)
    };
    if stored < width * channels {
        return Err(Error::Wav(format!(
            "WAV block align {stored} is smaller than {channels} channels of {bps}-bit samples"
        )));
    }
    let frames = data.len() / stored;
    let mut out = Vec::with_capacity(frames.saturating_mul(channels));
    for frame in 0..frames {
        let base = frame * stored;
        for channel in 0..channels {
            let at = base + channel * width;
            out.push(decode_one(fmt.format, fmt.bits, &data[at..at + width]));
        }
    }
    Ok(out)
}

fn decode_one(format: SampleEncoding, bits: u16, bytes: &[u8]) -> f32 {
    match (format, bits) {
        (SampleEncoding::Pcm, 8) => (f32::from(bytes[0]) - 128.0) / 128.0,
        (SampleEncoding::Pcm, 16) => {
            let sample = i16::from_le_bytes([bytes[0], bytes[1]]);
            f32::from(sample) / 32768.0
        }
        (SampleEncoding::Pcm, 24) => {
            let raw =
                i32::from(bytes[0]) | (i32::from(bytes[1]) << 8) | (i32::from(bytes[2]) << 16);
            let signed = if raw & 0x80_0000 != 0 {
                raw | !0xFF_FFFF
            } else {
                raw
            };
            signed as f32 / 8_388_608.0
        }
        (SampleEncoding::Float, 32) => {
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if sample.is_finite() {
                sample
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

fn signed_to_wav8(sample: u8) -> u8 {
    (i16::from(sample as i8) + 128) as u8
}

fn read_array(bytes: &[u8], offset: usize) -> Result<[u8; 4], Error> {
    let end = offset + 4;
    if end > bytes.len() {
        return Err(Error::Wav(
            "WAV file ended in the middle of a chunk header".to_string(),
        ));
    }
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&bytes[offset..end]);
    Ok(buf)
}

fn chunk_name(id: &[u8]) -> String {
    if id.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        id.iter().map(|b| char::from(*b)).collect()
    } else {
        format!("{:02X}{:02X}{:02X}{:02X}", id[0], id[1], id[2], id[3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_describes_stereo_pcm() {
        let bytes = wav_bytes(44_100, &[0, -1, 1, 2]).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            44_100
        );
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(bytes[44..46].try_into().unwrap()), 0);
        assert_eq!(i16::from_le_bytes(bytes[46..48].try_into().unwrap()), -1);
        assert_eq!(bytes.len(), 52);
    }

    #[test]
    fn mono8_export_is_unsigned_mono() {
        let signed = [0u8, 127, 128, 255];
        let bytes = encode_mono8_wav(8_287, &signed).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 8_287);
        assert_eq!(&bytes[44..], &[128, 255, 0, 127]);
        let decoded = decode_wav(&bytes).unwrap();
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.sample_rate, 8_287);
        assert!(decoded.interleaved[0].abs() < 1e-6);
        assert!((decoded.interleaved[1] - 127.0 / 128.0).abs() < 1e-6);
        assert!((decoded.interleaved[2] + 1.0).abs() < 1e-6);
        assert!((decoded.interleaved[3] + 1.0 / 128.0).abs() < 1e-6);
    }

    #[test]
    fn decodes_16_bit_stereo_and_24_bit_and_float() {
        let stereo = synth_wav(1, 2, 44_100, 16, &i16_bytes(&[0, i16::MAX, i16::MIN, 256]));
        let decoded = decode_wav(&stereo).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.interleaved.len(), 4);
        assert!(decoded.interleaved[0].abs() < 1e-6);
        assert!((decoded.interleaved[1] - 32767.0 / 32768.0).abs() < 1e-6);
        assert!((decoded.interleaved[2] + 1.0).abs() < 1e-6);

        let mut pcm24 = Vec::new();
        pcm24.extend_from_slice(&0x7F_FFFFu32.to_le_bytes()[..3]);
        pcm24.extend_from_slice(&0x80_0000u32.to_le_bytes()[..3]);
        let wav24 = synth_wav(1, 1, 48_000, 24, &pcm24);
        let decoded = decode_wav(&wav24).unwrap();
        assert!((decoded.interleaved[0] - 1.0).abs() < 1e-5);
        assert!((decoded.interleaved[1] + 1.0).abs() < 1e-6);

        let mut float_bytes = Vec::new();
        float_bytes.extend_from_slice(&0.5f32.to_le_bytes());
        float_bytes.extend_from_slice(&f32::NAN.to_le_bytes());
        let wavf = synth_wav_format(3, 1, 22_050, 32, &float_bytes);
        let decoded = decode_wav(&wavf).unwrap();
        assert!((decoded.interleaved[0] - 0.5).abs() < 1e-6);
        assert_eq!(decoded.interleaved[1], 0.0);
    }

    #[test]
    fn eight_bit_wav_is_unsigned() {
        let wav = synth_wav(1, 1, 8_000, 8, &[0, 128, 255]);
        let decoded = decode_wav(&wav).unwrap();
        assert!((decoded.interleaved[0] + 1.0).abs() < 1e-6);
        assert!(decoded.interleaved[1].abs() < 1e-6);
        assert!((decoded.interleaved[2] - 127.0 / 128.0).abs() < 1e-6);
    }

    #[test]
    fn extensible_pcm_is_accepted() {
        let payload = i16_bytes(&[0, 1000]);
        let wav = synth_extensible(1, 1, 16_000, 16, &payload);
        let decoded = decode_wav(&wav).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.interleaved.len(), 2);
        assert!(decoded.interleaved[0].abs() < 1e-6);
    }

    #[test]
    fn unsupported_and_truncated_files_name_the_problem() {
        let err = decode_wav(b"not a wav file!!").unwrap_err();
        assert!(err.to_string().contains("RIFF"), "{err}");

        let adpcm = synth_wav_format(2, 1, 8_000, 4, &[0, 1, 2, 3]);
        let err = decode_wav(&adpcm).unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");

        let weird = synth_wav(1, 1, 8_000, 32, &[0, 0, 0, 0]);
        let err = decode_wav(&weird).unwrap_err();
        assert!(err.to_string().contains("32-bit PCM"), "{err}");

        let err = decode_wav(b"RIFF").unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");

        let mut header = synth_wav(1, 1, 8_000, 16, &[0, 0]);
        header.truncate(20);
        let err = decode_wav(&header).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("truncated") || text.contains("no fmt") || text.contains("no data"),
            "{text}"
        );
    }

    fn i16_bytes(samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    fn synth_wav(format: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        synth_wav_format(format, channels, rate, bits, data)
    }

    fn synth_wav_format(format: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let block = channels * (bits / 8);
        let byte_rate = rate * u32::from(block);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&format.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        wrap_wave(&[chunk(b"fmt ", &fmt), chunk(b"data", data)])
    }

    fn synth_extensible(
        subformat: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let block = channels * (bits / 8);
        let byte_rate = rate * u32::from(block);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&0u32.to_le_bytes());
        fmt.extend_from_slice(&subformat.to_le_bytes());
        fmt.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
        ]);
        wrap_wave(&[chunk(b"fmt ", &fmt), chunk(b"data", data)])
    }

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn wrap_wave(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for chunk in chunks {
            body.extend_from_slice(chunk);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }
}
