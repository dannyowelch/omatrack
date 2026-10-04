//! WAV import and export, generated in the test so no audio is committed.

use omatrack::sample_edit;
use omatrack::{c2_rate, decode_wav, encode_mono8_wav, import_pcm, ImportOptions, Module, Sample};

#[test]
fn generated_wav_survives_import_export_and_sample_edits_undo_at_the_document() {
    let wav = pcm16_mono(16_000, &ramp(48));
    let decoded = decode_wav(&wav).expect("generated wav");
    let imported = import_pcm(
        &decoded,
        &ImportOptions {
            target_rate: c2_rate(0),
            normalize: true,
            dither: false,
            dither_seed: 1,
        },
    )
    .unwrap();
    assert_eq!(imported.data.len() % 2, 0);
    assert!(imported.data.len() > 2);
    let peak = imported
        .data
        .iter()
        .map(|byte| (*byte as i8).unsigned_abs())
        .max()
        .unwrap();
    assert!(
        peak >= 127,
        "normalize should reach full scale, peak {peak}"
    );

    let exported = encode_mono8_wav(imported.target_rate, &imported.data).unwrap();
    let again = decode_wav(&exported).unwrap();
    let round = import_pcm(
        &again,
        &ImportOptions {
            target_rate: again.sample_rate,
            normalize: false,
            dither: false,
            dither_seed: 1,
        },
    )
    .unwrap();
    assert_eq!(round.data, imported.data);

    let mut sample = Sample::default();
    sample.set_data(imported.data.clone()).unwrap();
    sample.volume = 64;
    let before = sample.clone();
    sample_edit::reverse(&mut sample);
    sample_edit::fade_out(&mut sample);
    assert_ne!(sample.data, before.data);
    sample_edit::clear_data(&mut sample);
    assert!(sample.data.is_empty());
    sample = before;
    assert!(!sample.data.is_empty());

    let mut module = Module::default();
    module.samples[0] = sample.clone();
    sample_edit::copy_to(&mut module, 0, 4).unwrap();
    assert_eq!(module.samples[4].data, sample.data);
}

fn ramp(frames: usize) -> Vec<i16> {
    (0..frames)
        .map(|index| {
            let turn = (index as i32 - frames as i32 / 2) * 400;
            turn.clamp(-12_000, 12_000) as i16
        })
        .collect()
}

fn pcm16_mono(rate: u32, samples: &[i16]) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let mut body = Vec::new();
    for (id, chunk) in [
        (b"fmt ".as_slice(), fmt.as_slice()),
        (b"data", data.as_slice()),
    ] {
        body.extend_from_slice(id);
        body.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        body.extend_from_slice(chunk);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&body);
    out
}
