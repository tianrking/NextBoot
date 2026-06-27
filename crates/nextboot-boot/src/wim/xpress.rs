use super::{
    XpressDecodeError, HUFFMAN_BITS, XPRESS_BLOCK_SIZE, XPRESS_CODE_COUNT, XPRESS_END_MARKER,
    XPRESS_LENGTH_TABLE_SIZE,
};

pub fn decompress_xpress(input: &[u8], out: &mut [u8]) -> Result<usize, XpressDecodeError> {
    if out.is_empty() {
        return Ok(0);
    }

    let mut output_len = 0usize;
    let mut next_block_at = 0usize;
    let mut alphabet = XpressHuffman::empty();
    let mut bitstream = XpressBitstream::empty(input);

    loop {
        if output_len >= out.len() {
            return Ok(output_len);
        }

        if output_len >= next_block_at {
            let position = bitstream.position;
            let lengths_end = position
                .checked_add(XPRESS_LENGTH_TABLE_SIZE)
                .ok_or(XpressDecodeError::InputTooShort)?;
            let length_bytes = input
                .get(position..lengths_end)
                .ok_or(XpressDecodeError::InputTooShort)?;
            alphabet = XpressHuffman::from_length_bytes(length_bytes)?;
            bitstream = XpressBitstream::new(input, lengths_end)?;
            next_block_at = output_len
                .checked_add(XPRESS_BLOCK_SIZE)
                .ok_or(XpressDecodeError::OutputOverflow)?;
        }

        let (raw, len) = alphabet.decode(bitstream.peek())?;
        bitstream.consume(len)?;

        if raw < XPRESS_END_MARKER {
            if output_len >= out.len() {
                return Err(XpressDecodeError::OutputOverflow);
            }
            out[output_len] = raw as u8;
            output_len += 1;
            continue;
        }

        if raw == XPRESS_END_MARKER && bitstream.position >= input.len().saturating_sub(1) {
            return Ok(output_len);
        }

        let raw = raw - XPRESS_END_MARKER;
        let match_offset_bits = raw >> 4;
        let mut match_len = usize::from(raw & 0x0f);
        if match_len == 0x0f {
            match_len = usize::from(bitstream.read_u8()?);
            if match_len == 0xff {
                match_len = usize::from(bitstream.read_u16()?);
            } else {
                match_len += 0x0f;
            }
        }
        match_len = match_len
            .checked_add(3)
            .ok_or(XpressDecodeError::OutputOverflow)?;

        let match_offset = if match_offset_bits == 0 {
            1usize
        } else {
            let bits = u8::try_from(match_offset_bits)
                .map_err(|_| XpressDecodeError::InvalidHuffmanCode)?;
            bitstream.read_offset(bits)?
        };

        if match_offset == 0 || match_offset > output_len {
            return Err(XpressDecodeError::InvalidMatchOffset);
        }
        let end = output_len
            .checked_add(match_len)
            .ok_or(XpressDecodeError::OutputOverflow)?;
        if end > out.len() {
            return Err(XpressDecodeError::OutputOverflow);
        }
        for _ in 0..match_len {
            let byte = out[output_len - match_offset];
            out[output_len] = byte;
            output_len += 1;
        }
    }
}

struct XpressHuffman {
    counts: [u16; HUFFMAN_BITS + 1],
    starts: [u32; HUFFMAN_BITS + 1],
    first_symbol: [usize; HUFFMAN_BITS + 1],
    symbols: [u16; XPRESS_CODE_COUNT],
}

impl XpressHuffman {
    fn empty() -> Self {
        Self {
            counts: [0; HUFFMAN_BITS + 1],
            starts: [0; HUFFMAN_BITS + 1],
            first_symbol: [0; HUFFMAN_BITS + 1],
            symbols: [0; XPRESS_CODE_COUNT],
        }
    }

    fn from_length_bytes(length_bytes: &[u8]) -> Result<Self, XpressDecodeError> {
        if length_bytes.len() != XPRESS_LENGTH_TABLE_SIZE {
            return Err(XpressDecodeError::InputTooShort);
        }

        let mut out = Self::empty();
        let mut lengths = [0u8; XPRESS_CODE_COUNT];
        let mut non_empty = false;
        for symbol in 0..XPRESS_CODE_COUNT {
            let byte = length_bytes[symbol / 2];
            let len = if symbol % 2 == 0 {
                byte & 0x0f
            } else {
                byte >> 4
            };
            if usize::from(len) > HUFFMAN_BITS {
                return Err(XpressDecodeError::InvalidHuffmanLength);
            }
            lengths[symbol] = len;
            if len != 0 {
                out.counts[usize::from(len)] += 1;
                non_empty = true;
            }
        }

        if !non_empty {
            return Err(XpressDecodeError::IncompleteHuffmanAlphabet);
        }

        let mut huf = 0u32;
        let mut cumulative = 0usize;
        for bits in 1..=HUFFMAN_BITS {
            out.starts[bits] = huf << (HUFFMAN_BITS - bits);
            out.first_symbol[bits] = cumulative;
            huf = huf
                .checked_add(u32::from(out.counts[bits]))
                .ok_or(XpressDecodeError::IncompleteHuffmanAlphabet)?;
            if huf > (1u32 << bits) {
                return Err(XpressDecodeError::IncompleteHuffmanAlphabet);
            }
            huf <<= 1;
            cumulative = cumulative
                .checked_add(usize::from(out.counts[bits]))
                .ok_or(XpressDecodeError::IncompleteHuffmanAlphabet)?;
        }
        if huf != (1u32 << (HUFFMAN_BITS + 1)) {
            return Err(XpressDecodeError::IncompleteHuffmanAlphabet);
        }

        let mut cursor = out.first_symbol;
        for (symbol, len) in lengths.iter().copied().enumerate() {
            if len == 0 {
                continue;
            }
            let len = usize::from(len);
            out.symbols[cursor[len]] = symbol as u16;
            cursor[len] += 1;
        }

        Ok(out)
    }

    fn decode(&self, huf: u32) -> Result<(u16, u8), XpressDecodeError> {
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

        Err(XpressDecodeError::InvalidHuffmanCode)
    }
}

struct XpressBitstream<'a> {
    input: &'a [u8],
    position: usize,
    accum: u32,
    extra_bits: i32,
}

impl<'a> XpressBitstream<'a> {
    fn empty(input: &'a [u8]) -> Self {
        Self {
            input,
            position: 0,
            accum: 0,
            extra_bits: 0,
        }
    }

    fn new(input: &'a [u8], position: usize) -> Result<Self, XpressDecodeError> {
        let mut out = Self {
            input,
            position,
            accum: 0,
            extra_bits: 16,
        };
        let high = u32::from(out.read_u16()?);
        let low = u32::from(out.read_u16()?);
        out.accum = (high << 16) | low;
        Ok(out)
    }

    fn peek(&self) -> u32 {
        self.accum >> (32 - HUFFMAN_BITS)
    }

    fn consume(&mut self, bits: u8) -> Result<(), XpressDecodeError> {
        if bits == 0 || usize::from(bits) > HUFFMAN_BITS {
            return Err(XpressDecodeError::InvalidHuffmanCode);
        }
        self.accum <<= u32::from(bits);
        self.extra_bits -= i32::from(bits);
        if self.extra_bits < 0 {
            let shift = u32::try_from(-self.extra_bits)
                .map_err(|_| XpressDecodeError::InvalidHuffmanCode)?;
            let word = u32::from(self.read_u16()?);
            self.accum |= word << shift;
            self.extra_bits += 16;
        }
        Ok(())
    }

    fn read_offset(&mut self, bits: u8) -> Result<usize, XpressDecodeError> {
        if bits == 0 || usize::from(bits) > HUFFMAN_BITS {
            return Err(XpressDecodeError::InvalidHuffmanCode);
        }
        let value = (self.accum >> (32 - u32::from(bits))) + (1u32 << bits);
        self.accum <<= u32::from(bits);
        self.extra_bits -= i32::from(bits);
        if self.extra_bits < 0 {
            let shift = u32::try_from(-self.extra_bits)
                .map_err(|_| XpressDecodeError::InvalidHuffmanCode)?;
            let word = u32::from(self.read_u16()?);
            self.accum |= word << shift;
            self.extra_bits += 16;
        }
        usize::try_from(value).map_err(|_| XpressDecodeError::InvalidHuffmanCode)
    }

    fn read_u8(&mut self) -> Result<u8, XpressDecodeError> {
        let byte = *self
            .input
            .get(self.position)
            .ok_or(XpressDecodeError::InputTooShort)?;
        self.position += 1;
        Ok(byte)
    }

    fn read_u16(&mut self) -> Result<u16, XpressDecodeError> {
        let bytes = self
            .input
            .get(self.position..self.position + 2)
            .ok_or(XpressDecodeError::InputTooShort)?;
        self.position += 2;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}
