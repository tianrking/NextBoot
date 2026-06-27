use super::*;
use alloc::vec::Vec;

#[test]
fn decompresses_uncompressed_block() {
    let compressed = make_lzx_uncompressed_block(b"hello");
    let mut out = [0u8; 5];

    assert_eq!(decompress_lzx(&compressed, &mut out), Ok(5));
    assert_eq!(&out, b"hello");
}

#[test]
fn rejects_odd_length_input() {
    let mut out = [0u8; 1];

    assert_eq!(
        decompress_lzx(&[0, 1, 2], &mut out),
        Err(LzxDecodeError::OddInputLength)
    );
}

#[test]
fn decompresses_verbatim_literal_block() {
    let compressed = make_lzx_verbatim_literal_block();
    let mut out = [0u8; 2];

    assert_eq!(decompress_lzx(&compressed, &mut out), Ok(2));
    assert_eq!(&out, b"hi");
}

fn make_lzx_uncompressed_block(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::new();
    push_bits(&mut bits, 3, 3);
    push_bits(&mut bits, 0, 1);
    push_bits(&mut bits, ((bytes.len() >> 8) & 0xff) as u16, 8);
    push_bits(&mut bits, (bytes.len() & 0xff) as u16, 8);
    push_bits(&mut bits, 0, 1);
    while bits.len() % 16 != 0 {
        bits.push(0);
    }

    let mut out = bits_to_le_words(&bits);
    for _ in 0..LZX_REPEATED_OFFSETS {
        out.extend_from_slice(&1u32.to_le_bytes());
    }
    out.extend_from_slice(bytes);
    if bytes.len() % 2 != 0 {
        out.push(0);
    }
    out
}

fn make_lzx_verbatim_literal_block() -> Vec<u8> {
    let mut bits = Vec::new();
    push_bits(&mut bits, 1, 3);
    push_bits(&mut bits, 0, 1);
    push_bits(&mut bits, 0, 8);
    push_bits(&mut bits, 2, 8);

    push_pretree_lengths(&mut bits, &[0, 16]);
    for symbol in 0..LZX_MAIN_LIT_CODES {
        bits.push(u8::from(
            symbol == usize::from(b'h') || symbol == usize::from(b'i'),
        ));
    }

    push_pretree_lengths(&mut bits, &[0, 1]);
    for _ in 0..(LZX_MAIN_CODES - LZX_MAIN_LIT_CODES) {
        bits.push(0);
    }

    push_pretree_lengths(&mut bits, &[0, 1]);
    for _ in 0..LZX_LENGTH_CODES {
        bits.push(0);
    }

    bits.push(0);
    bits.push(1);
    while bits.len() % 16 != 0 {
        bits.push(0);
    }

    bits_to_le_words(&bits)
}

fn push_pretree_lengths(bits: &mut Vec<u8>, active_codes: &[usize]) {
    for code in 0..LZX_PRETREE_CODES {
        push_bits(bits, u16::from(active_codes.contains(&code)), 4);
    }
}

fn push_bits(bits: &mut Vec<u8>, value: u16, count: usize) {
    for index in (0..count).rev() {
        bits.push(((value >> index) & 1) as u8);
    }
}

fn bits_to_le_words(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in bits.chunks_exact(16) {
        let mut word = 0u16;
        for bit in chunk {
            word = (word << 1) | u16::from(*bit);
        }
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}
