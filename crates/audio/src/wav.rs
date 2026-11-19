//! WAV (RIFF WAVE) writing via `craft-audio`.

pub use craft_audio::wav_stream::{Bits, append, header};

/// A whole WAV file in memory (empty when it would not fit a WAV's 4 GB limit).
pub fn encode(rate: u32, channels: u16, samples: &[f32], bits: Bits) -> Vec<u8> {
    let depth = match bits {
        Bits::Int16 => craft_audio::BitDepth::Int16,
        Bits::Int24 => craft_audio::BitDepth::Int24,
        Bits::Float32 => craft_audio::BitDepth::Float32,
    };
    let buf = craft_audio::AudioBuffer::from_interleaved(rate, channels, samples);
    let opts = craft_audio::EncodeOptions::new(craft_audio::EncodeFormat::Wav).with_bit_depth(depth);
    craft_audio::encode(&buf, &opts).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{AudioError, decode};

    const PCM: u16 = 1;
    const EXTENSIBLE: u16 = 0xFFFE;

    #[test]
    fn streaming_blocks_round_trip_all_formats() {
        let samples = [0.25, -0.5, 0.0, 0.75, -1.0, 0.5];
        for bits in [Bits::Int16, Bits::Int24, Bits::Float32] {
            let mut bytes = header(48_000, 2, bits, 3).unwrap();
            for block in samples.chunks(2) {
                append(&mut bytes, block, bits);
            }
            let decoded = decode(&bytes).unwrap();
            assert_eq!((decoded.rate, decoded.channels, decoded.frames()), (48_000, 2, 3));
            assert_eq!(decoded.samples, samples);
        }
    }

    #[test]
    fn sixteen_bit_round_trips_bit_exactly() {
        let src: Vec<i16> = vec![0, 1, -1, 32_767, -32_768, 12_345, -54, 7];
        let f: Vec<f32> = src.iter().map(|&v| f32::from(v) / 32_768.0).collect();
        let wav = encode(22_050, 2, &f, Bits::Int16);
        let p = decode(&wav).unwrap();
        assert_eq!((p.rate, p.channels, p.frames()), (22_050, 2, 4));
        assert_eq!(p.samples, f);
        let again = encode(22_050, 2, &p.samples, Bits::Int16);
        assert_eq!(again, wav, "bit-identical after a round trip");
    }

    #[test]
    fn every_sample_format_reads_to_the_same_values() {
        let vals = [0.0f32, 0.5, -0.5, 0.25];
        for bits in [Bits::Int16, Bits::Int24, Bits::Float32] {
            let p = decode(&encode(48_000, 1, &vals, bits)).unwrap();
            for (a, b) in p.samples.iter().zip(vals) {
                assert!((a - b).abs() < 1e-4, "{bits:?}: {a} vs {b}");
            }
        }
        // 8-bit unsigned, written by hand: 128 is silence.
        let mut w = header(8000, 1, Bits::Int16, 0).unwrap();
        w[34] = 8; // bits per sample
        w[32] = 1; // block align
        w.truncate(40);
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[128, 255, 0, 192]);
        let p = decode(&w).unwrap();
        assert_eq!(p.samples, vec![0.0, 127.0 / 128.0, -1.0, 0.5]);
    }

    #[test]
    fn extensible_surround_keeps_front_left_and_right_and_skips_unknown_chunks() {
        // 6 channels of 16-bit in a WAVE_FORMAT_EXTENSIBLE fmt, after a LIST chunk of odd size.
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        w.extend_from_slice(b"LIST");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(b"abc\0"); // odd size: one pad byte
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&40u32.to_le_bytes());
        w.extend_from_slice(&EXTENSIBLE.to_le_bytes());
        w.extend_from_slice(&6u16.to_le_bytes());
        w.extend_from_slice(&44_100u32.to_le_bytes());
        w.extend_from_slice(&(44_100u32 * 12).to_le_bytes());
        w.extend_from_slice(&12u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(&22u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(&0x3fu32.to_le_bytes());
        w.extend_from_slice(&PCM.to_le_bytes());
        w.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&12u32.to_le_bytes());
        for v in [16_384i16, -16_384, 1, 2, 3, 4] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        let p = decode(&w).unwrap();
        assert_eq!((p.channels, p.samples.clone()), (2, vec![0.5, -0.5]));
    }

    #[test]
    fn unsupported_and_damaged_files_say_so() {
        let mut w = header(8000, 1, Bits::Int16, 4).unwrap();
        append(&mut w, &[0.0; 4], Bits::Int16);
        w[20] = 99; // unknown WAV codec tag
        assert!(matches!(decode(&w), Err(AudioError::Unsupported(_) | AudioError::Unreadable)));
        let mut ok = header(8000, 1, Bits::Int16, 4).unwrap();
        append(&mut ok, &[0.0; 4], Bits::Int16);
        assert!(decode(&ok[..36]).is_err(), "no data chunk");
        assert_eq!(decode(&ok[..20]), Err(AudioError::Unreadable), "fmt cut short");
        // A data chunk claiming more than there is reads what is there.
        let mut long = ok.clone();
        long[40..44].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert_eq!(decode(&long).unwrap().frames(), 4);
        assert!(header(8000, 2, Bits::Float32, u64::MAX / 2).is_none(), "past the 4 GB limit");
    }
}
