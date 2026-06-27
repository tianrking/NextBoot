use super::{LzxDecodeError, HUFFMAN_BITS};

pub(super) struct LzxHuffman<const N: usize> {
    counts: [u16; HUFFMAN_BITS + 1],
    starts: [u32; HUFFMAN_BITS + 1],
    first_symbol: [usize; HUFFMAN_BITS + 1],
    symbols: [u16; N],
}

impl<const N: usize> LzxHuffman<N> {
    pub(super) fn empty() -> Self {
        Self {
            counts: [0; HUFFMAN_BITS + 1],
            starts: [0; HUFFMAN_BITS + 1],
            first_symbol: [0; HUFFMAN_BITS + 1],
            symbols: [0; N],
        }
    }

    pub(super) fn from_lengths(lengths: &[u8; N]) -> Result<Self, LzxDecodeError> {
        let mut out = Self::empty();
        let mut empty = true;
        for len in lengths {
            let len = usize::from(*len);
            if len > HUFFMAN_BITS {
                return Err(LzxDecodeError::InvalidHuffmanLength);
            }
            if len != 0 {
                out.counts[len] += 1;
                empty = false;
            }
        }
        if empty {
            out.counts[1] = 2;
        }

        let mut huf = 0u32;
        let mut cumulative = 0usize;
        for bits in 1..=HUFFMAN_BITS {
            out.starts[bits] = huf << (HUFFMAN_BITS - bits);
            out.first_symbol[bits] = cumulative;
            huf = huf
                .checked_add(u32::from(out.counts[bits]))
                .ok_or(LzxDecodeError::IncompleteHuffmanAlphabet)?;
            if huf > (1u32 << bits) {
                return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
            }
            huf <<= 1;
            cumulative = cumulative
                .checked_add(usize::from(out.counts[bits]))
                .ok_or(LzxDecodeError::IncompleteHuffmanAlphabet)?;
            if cumulative > N {
                return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
            }
        }
        if huf != (1u32 << (HUFFMAN_BITS + 1)) {
            return Err(LzxDecodeError::IncompleteHuffmanAlphabet);
        }

        if !empty {
            let mut cursor = out.first_symbol;
            for (symbol, len) in lengths.iter().copied().enumerate() {
                if len == 0 {
                    continue;
                }
                let len = usize::from(len);
                out.symbols[cursor[len]] = symbol as u16;
                cursor[len] += 1;
            }
        }

        Ok(out)
    }

    pub(super) fn decode(&self, huf: u32) -> Result<(u16, u8), LzxDecodeError> {
        for bits in 1..=HUFFMAN_BITS {
            let count = u32::from(self.counts[bits]);
            if count == 0 {
                continue;
            }
            let shift = HUFFMAN_BITS - bits;
            let start = self.starts[bits];
            let end = start + (count << shift);
            if huf >= start && huf < end {
                let index = self.first_symbol[bits] + ((huf - start) >> shift) as usize;
                return Ok((self.symbols[index], bits as u8));
            }
        }

        Err(LzxDecodeError::InvalidHuffmanCode)
    }
}
