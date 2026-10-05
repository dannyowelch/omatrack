//! ProTracker `.mod` reader and writer.
//!
//! 31-sample, 4-channel layout:
//!
//! ```text
//! 0      20    title
//! 20     930   31 × 30-byte sample headers
//! 950    1     song length (1–128)
//! 951    1     restart position
//! 952    128   order list
//! 1080   4     tag (M.K., M!K!, FLT4, or 4CHN)
//! 1084   …     patterns, 1024 bytes each (64 rows × 4 channels × 4 bytes)
//! …      …     sample data, signed 8-bit, even length
//! ```
//!
//! Pattern count is `max(order) + 1` across all 128 order bytes. Each cell is:
//!
//! ```text
//! byte 0: sample high nibble | period bits 8–11
//! byte 1: period bits 0–7
//! byte 2: sample low nibble  | effect
//! byte 3: effect parameter
//! ```

use std::fs;
use std::path::Path;

use crate::error::{Error, IoKind};
use crate::module::{
    validate_sample_data, Cell, Module, Pattern, Sample, Tag, CHANNELS, ORDER_LEN, ROWS,
    SAMPLE_COUNT, SAMPLE_NAME_LEN, TITLE_LEN,
};

/// Bytes before the first pattern.
pub const HEADER_LEN: usize = 20 + SAMPLE_COUNT * 30 + 1 + 1 + ORDER_LEN + 4;
/// Bytes in one pattern.
pub const PATTERN_BYTES: usize = ROWS * CHANNELS * 4;

const _: () = assert!(HEADER_LEN == 1084);
const _: () = assert!(PATTERN_BYTES == 1024);

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, len: usize, context: &'static str) -> Result<&'a [u8], Error> {
        let Some(end) = self.pos.checked_add(len) else {
            return Err(Error::Truncated {
                context,
                expected: usize::MAX,
                actual: self.data.len(),
            });
        };
        if end > self.data.len() {
            return Err(Error::Truncated {
                context,
                expected: end,
                actual: self.data.len(),
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn bytes<const N: usize>(&mut self, context: &'static str) -> Result<[u8; N], Error> {
        let slice = self.take(N, context)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u8(&mut self, context: &'static str) -> Result<u8, Error> {
        Ok(self.take(1, context)?[0])
    }

    fn u16_be(&mut self, context: &'static str) -> Result<u16, Error> {
        let bytes = self.bytes::<2>(context)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }
}

impl Module {
    /// Parse a 31-sample 4-channel `.mod`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        parse(bytes)
    }

    /// Encode a module. Fails if the document cannot be stored losslessly.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        write(self)
    }

    /// Read a `.mod` from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let bytes =
            fs::read(path).map_err(|source| Error::io(IoKind::Read, path.to_path_buf(), source))?;
        parse(&bytes)
    }

    /// Write a `.mod` to disk.
    ///
    /// Fails when [`Self::patterns`] is shorter or longer than
    /// [`Self::required_pattern_count`]. Use [`Self::save_stored`] to write the
    /// prefix a `.mod` can address and keep any higher patterns in memory.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let bytes = write(self)?;
        fs::write(path, bytes)
            .map_err(|source| Error::io(IoKind::Write, path.to_path_buf(), source))?;
        Ok(())
    }

    /// Write the patterns a `.mod` can store, without dropping the rest.
    ///
    /// Pattern count in the file is [`Self::required_pattern_count`]
    /// (`max(order) + 1` over all 128 order bytes). Patterns in that range are
    /// written even when the played song does not use every one. A pattern
    /// index above that has no slot in the file. Those patterns stay on
    /// `self`. The returned index is the first pattern that was not written,
    /// when the in-memory list is longer than the file can hold.
    pub fn save_stored(&self, path: impl AsRef<Path>) -> Result<Option<usize>, Error> {
        let path = path.as_ref();
        let (bytes, omitted) = stored_bytes(self)?;
        fs::write(path, bytes)
            .map_err(|source| Error::io(IoKind::Write, path.to_path_buf(), source))?;
        Ok(omitted)
    }
}

fn stored_bytes(module: &Module) -> Result<(Vec<u8>, Option<usize>), Error> {
    let expected = module.required_pattern_count();
    if module.patterns.len() < expected {
        return Err(Error::PatternCount {
            expected,
            actual: module.patterns.len(),
        });
    }
    if module.patterns.len() == expected {
        return Ok((write(module)?, None));
    }
    let mut copy = module.clone();
    copy.patterns.truncate(expected);
    Ok((write(&copy)?, Some(expected)))
}

fn parse(bytes: &[u8]) -> Result<Module, Error> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::Truncated {
            context: "header",
            expected: HEADER_LEN,
            actual: bytes.len(),
        });
    }
    let mut reader = Reader::new(bytes);
    let title = reader.bytes::<TITLE_LEN>("header")?;

    let mut samples: [Sample; SAMPLE_COUNT] = std::array::from_fn(|_| Sample::default());
    let mut lengths = [0usize; SAMPLE_COUNT];
    for index in 0..SAMPLE_COUNT {
        let name = reader.bytes::<SAMPLE_NAME_LEN>("sample header")?;
        let words = reader.u16_be("sample header")?;
        let finetune_raw = reader.u8("sample header")?;
        let volume = reader.u8("sample header")?;
        let loop_start = reader.u16_be("sample header")?;
        let loop_length = reader.u16_be("sample header")?;
        lengths[index] = usize::from(words) * 2;
        samples[index] = Sample {
            name,
            finetune_raw,
            volume,
            loop_start,
            loop_length,
            data: Vec::new(),
        };
    }

    let song_length = reader.u8("header")?;
    let restart = reader.u8("header")?;
    let order = reader.bytes::<ORDER_LEN>("header")?;
    let tag = Tag::parse(reader.bytes::<4>("header")?)?;
    if !(1..=128).contains(&song_length) {
        return Err(Error::InvalidSongLength(song_length));
    }

    let pattern_count = usize::from(order.iter().copied().max().unwrap_or(0)) + 1;
    let pattern_bytes = pattern_count.saturating_mul(PATTERN_BYTES);
    require_remaining(&reader, pattern_bytes, "pattern data")?;
    let mut patterns = Vec::with_capacity(pattern_count);
    for _ in 0..pattern_count {
        patterns.push(parse_pattern(&mut reader)?);
    }

    let sample_bytes: usize = lengths.iter().sum();
    require_remaining(&reader, sample_bytes, "sample data")?;
    for (index, sample) in samples.iter_mut().enumerate() {
        let data = reader.take(lengths[index], "sample data")?.to_vec();
        sample.data = data;
    }

    Ok(Module {
        title,
        samples,
        song_length,
        restart,
        order,
        tag,
        patterns,
        trailing: reader.rest().to_vec(),
    })
}

fn require_remaining(reader: &Reader<'_>, len: usize, context: &'static str) -> Result<(), Error> {
    let Some(end) = reader.pos.checked_add(len) else {
        return Err(Error::Truncated {
            context,
            expected: usize::MAX,
            actual: reader.data.len(),
        });
    };
    if end > reader.data.len() {
        return Err(Error::Truncated {
            context,
            expected: end,
            actual: reader.data.len(),
        });
    }
    Ok(())
}

fn parse_pattern(reader: &mut Reader<'_>) -> Result<Pattern, Error> {
    let mut pattern = Pattern::empty();
    for row in &mut pattern.rows {
        for cell in row {
            let bytes = reader.bytes::<4>("pattern data")?;
            *cell = unpack_cell(bytes);
        }
    }
    Ok(pattern)
}

fn unpack_cell(bytes: [u8; 4]) -> Cell {
    let sample = (bytes[0] & 0xF0) | (bytes[2] >> 4);
    let period = (u16::from(bytes[0] & 0x0F) << 8) | u16::from(bytes[1]);
    let effect = bytes[2] & 0x0F;
    Cell {
        sample,
        period,
        effect,
        param: bytes[3],
    }
}

fn write(module: &Module) -> Result<Vec<u8>, Error> {
    if !(1..=128).contains(&module.song_length) {
        return Err(Error::InvalidSongLength(module.song_length));
    }
    let expected = module.required_pattern_count();
    if module.patterns.len() != expected {
        return Err(Error::PatternCount {
            expected,
            actual: module.patterns.len(),
        });
    }
    for (index, sample) in module.samples.iter().enumerate() {
        validate_sample_data(Some(index + 1), &sample.data)?;
    }

    let sample_bytes: usize = module.samples.iter().map(|sample| sample.data.len()).sum();
    let mut out = Vec::with_capacity(
        HEADER_LEN + module.patterns.len() * PATTERN_BYTES + sample_bytes + module.trailing.len(),
    );

    out.extend_from_slice(&module.title);
    for (index, sample) in module.samples.iter().enumerate() {
        out.extend_from_slice(&sample.name);
        let Ok(words) = u16::try_from(sample.data.len() / 2) else {
            return Err(Error::SampleTooLarge {
                sample: Some(index + 1),
                bytes: sample.data.len(),
            });
        };
        out.extend_from_slice(&words.to_be_bytes());
        out.push(sample.finetune_raw);
        out.push(sample.volume);
        out.extend_from_slice(&sample.loop_start.to_be_bytes());
        out.extend_from_slice(&sample.loop_length.to_be_bytes());
    }
    out.push(module.song_length);
    out.push(module.restart);
    out.extend_from_slice(&module.order);
    out.extend_from_slice(&module.tag.as_bytes());

    for pattern in &module.patterns {
        for row in &pattern.rows {
            for cell in row {
                out.extend_from_slice(&pack_cell(cell)?);
            }
        }
    }
    for sample in &module.samples {
        out.extend_from_slice(&sample.data);
    }
    out.extend_from_slice(&module.trailing);
    Ok(out)
}

fn pack_cell(cell: &Cell) -> Result<[u8; 4], Error> {
    if cell.period > 0x0FFF {
        return Err(Error::InvalidCell {
            field: "period",
            value: cell.period,
        });
    }
    if cell.effect > 0x0F {
        return Err(Error::InvalidCell {
            field: "effect",
            value: u16::from(cell.effect),
        });
    }
    let period_hi = ((cell.period >> 8) as u8) & 0x0F;
    let period_lo = cell.period as u8;
    let byte0 = (cell.sample & 0xF0) | period_hi;
    let byte2 = ((cell.sample & 0x0F) << 4) | cell.effect;
    Ok([byte0, period_lo, byte2, cell.param])
}
