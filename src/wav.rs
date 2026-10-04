//! 16-bit stereo PCM WAV files.
//!
//! The writer is intentionally small: playback tests and `omatrack --render`
//! need a header and little-endian samples, not a general audio library.

use std::fs;
use std::path::Path;

use crate::error::{Error, IoKind};

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
}
