//! Every encoder we expose must produce something its decoder can read.
//!
//! This is the property that makes an encoder an encoder, and it is the one a
//! hand-written test is worst at checking: a test author picks data, and the
//! data they pick tends to be regular. An encoder in this crate once shipped
//! with round-trip tests that all passed while it produced unreadable streams
//! for almost any real input, because every test compressed
//! `(0..n).map(|i| i % 256)` - strictly periodic bytes whose matches all sit
//! the same short distance back. Generated inputs do not have that blind spot.

#![cfg(any(
    feature = "lzma",
    feature = "deflate",
    feature = "bzip2",
    feature = "ppmd"
))]

// Which of these are needed depends on which codecs are compiled in.
#[allow(unused_imports)]
use std::io::{Read, Write};

use proptest::prelude::*;

/// Inputs across the shapes that break compressors in different ways:
/// incompressible noise, long runs, repeated structure, and short lengths.
#[allow(dead_code)]
fn data_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Arbitrary bytes, including the empty input.
        prop::collection::vec(any::<u8>(), 0..40_000),
        // Runs, which exercise long matches.
        prop::collection::vec((any::<u8>(), 1usize..500), 1..200).prop_map(|runs| runs
            .into_iter()
            .flat_map(|(b, n)| std::iter::repeat_n(b, n))
            .collect()),
        // Repeated blocks, which exercise distant matches - the case that
        // periodic test data never reaches.
        (prop::collection::vec(any::<u8>(), 16..2000), 2usize..30).prop_map(|(block, times)| {
            let mut out = Vec::with_capacity(block.len() * times);
            for _ in 0..times {
                out.extend_from_slice(&block);
            }
            out
        }),
    ]
}

#[cfg(feature = "lzma2")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn lzma2_round_trips(data in data_strategy()) {
        use zesven::codec::lzma::{Lzma2Decoder, Lzma2Encoder, Lzma2EncoderOptions};

        let options = Lzma2EncoderOptions { preset: 5, dict_size: Some(1 << 20) };
        let mut compressed = Vec::new();
        {
            let mut encoder = Lzma2Encoder::new(&mut compressed, &options);
            encoder.write_all(&data).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut decoder =
            Lzma2Decoder::new(std::io::Cursor::new(&compressed), &options.properties()).unwrap();
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        prop_assert_eq!(back, data);
    }
}

#[cfg(all(feature = "lzma2", feature = "parallel"))]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The multi-threaded encoder emits several independently coded chunks; a
    /// plain decoder must still read the concatenation.
    #[test]
    fn lzma2_multi_threaded_round_trips(data in data_strategy()) {
        use zesven::codec::lzma::{Lzma2Decoder, Lzma2EncoderMt, Lzma2EncoderOptions};

        let options = Lzma2EncoderOptions { preset: 5, dict_size: Some(1 << 16) };
        let mut compressed = Vec::new();
        {
            // A chunk size small enough that the generated inputs span several.
            let mut encoder =
                Lzma2EncoderMt::new(&mut compressed, &options, 1 << 16, 4).unwrap();
            encoder.write_all(&data).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut decoder =
            Lzma2Decoder::new(std::io::Cursor::new(&compressed), &options.properties()).unwrap();
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        prop_assert_eq!(back, data);
    }
}

#[cfg(feature = "lzma")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn lzma_round_trips(data in data_strategy()) {
        use zesven::codec::lzma::{LzmaDecoder, LzmaEncoder, LzmaEncoderOptions};

        let options = LzmaEncoderOptions { preset: 5, dict_size: Some(1 << 20) };
        let mut compressed = Vec::new();
        {
            let mut encoder = LzmaEncoder::new(&mut compressed, &options).unwrap();
            encoder.write_all(&data).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut decoder = LzmaDecoder::new(
            std::io::Cursor::new(&compressed),
            &options.properties(),
            data.len() as u64,
        )
        .unwrap();
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        prop_assert_eq!(back, data);
    }
}

#[cfg(feature = "deflate")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn deflate_round_trips(data in data_strategy()) {
        use zesven::codec::deflate::{DeflateDecoder, DeflateEncoder, DeflateEncoderOptions};

        let mut compressed = Vec::new();
        {
            let mut encoder =
                DeflateEncoder::new(&mut compressed, &DeflateEncoderOptions { level: 6 });
            encoder.write_all(&data).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut decoder = DeflateDecoder::new(std::io::Cursor::new(&compressed));
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        prop_assert_eq!(back, data);
    }
}

#[cfg(feature = "bzip2")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn bzip2_round_trips(data in data_strategy()) {
        use zesven::codec::bzip2::{Bzip2Decoder, Bzip2Encoder, Bzip2EncoderOptions};

        let mut compressed = Vec::new();
        {
            let mut encoder =
                Bzip2Encoder::new(&mut compressed, &Bzip2EncoderOptions { level: 5 });
            encoder.write_all(&data).unwrap();
            encoder.try_finish().unwrap();
        }

        let mut decoder = Bzip2Decoder::new(std::io::Cursor::new(&compressed));
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        prop_assert_eq!(back, data);
    }
}
