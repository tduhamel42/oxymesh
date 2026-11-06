//! Checked reading and writing on byte buffers, without panics or allocation.

/// The input is shorter than the announced structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated;

/// The output buffer is too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow;

/// Zero-copy read cursor. Every method fails cleanly at the end of the buffer.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    /// Number of bytes left to read.
    pub const fn remaining(&self) -> usize {
        self.buf.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// What is left, without consuming it.
    pub const fn peek_rest(&self) -> &'a [u8] {
        self.buf
    }

    /// Consumes everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        core::mem::take(&mut self.buf)
    }

    /// Consumes `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        let (head, tail) = self.buf.split_at_checked(n).ok_or(Truncated)?;
        self.buf = tail;
        Ok(head)
    }

    /// Consumes a fixed-size array.
    pub fn array<const N: usize>(&mut self) -> Result<&'a [u8; N], Truncated> {
        let (head, tail) = self.buf.split_first_chunk::<N>().ok_or(Truncated)?;
        self.buf = tail;
        Ok(head)
    }

    /// Consumes the last `N` bytes.
    pub fn array_back<const N: usize>(&mut self) -> Result<&'a [u8; N], Truncated> {
        let (head, tail) = self.buf.split_last_chunk::<N>().ok_or(Truncated)?;
        self.buf = head;
        Ok(tail)
    }

    pub fn u8(&mut self) -> Result<u8, Truncated> {
        let (&b, tail) = self.buf.split_first().ok_or(Truncated)?;
        self.buf = tail;
        Ok(b)
    }

    pub fn u16_le(&mut self) -> Result<u16, Truncated> {
        self.array::<2>().map(|a| u16::from_le_bytes(*a))
    }

    pub fn u32_le(&mut self) -> Result<u32, Truncated> {
        self.array::<4>().map(|a| u32::from_le_bytes(*a))
    }

    pub fn i32_le(&mut self) -> Result<i32, Truncated> {
        self.array::<4>().map(|a| i32::from_le_bytes(*a))
    }

    /// A length byte followed by that many bytes.
    pub fn len8(&mut self) -> Result<&'a [u8], Truncated> {
        let n = self.u8()?;
        self.take(usize::from(n))
    }
}

/// Write cursor over a caller-provided buffer.
#[derive(Debug)]
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Number of bytes written.
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// The bytes written so far.
    pub fn written(&self) -> &[u8] {
        self.buf.get(..self.pos).unwrap_or(&[])
    }

    pub fn put(&mut self, src: &[u8]) -> Result<(), Overflow> {
        let end = self.pos.checked_add(src.len()).ok_or(Overflow)?;
        let dst = self.buf.get_mut(self.pos..end).ok_or(Overflow)?;
        dst.copy_from_slice(src);
        self.pos = end;
        Ok(())
    }

    pub fn put_u8(&mut self, v: u8) -> Result<(), Overflow> {
        self.put(&[v])
    }

    pub fn put_u16_le(&mut self, v: u16) -> Result<(), Overflow> {
        self.put(&v.to_le_bytes())
    }

    pub fn put_u32_le(&mut self, v: u32) -> Result<(), Overflow> {
        self.put(&v.to_le_bytes())
    }

    pub fn put_i32_le(&mut self, v: i32) -> Result<(), Overflow> {
        self.put(&v.to_le_bytes())
    }

    /// A length byte, then the bytes. Fails if `src` exceeds 255 bytes.
    pub fn put_len8(&mut self, src: &[u8]) -> Result<(), Overflow> {
        let n = u8::try_from(src.len()).map_err(|_| Overflow)?;
        self.put_u8(n)?;
        self.put(src)
    }

    /// Reserves a byte to fill in later with [`Writer::patch_u8`]. Returns its position.
    pub fn reserve_u8(&mut self) -> Result<usize, Overflow> {
        let at = self.pos;
        self.put_u8(0)?;
        Ok(at)
    }

    /// Overwrites a byte that has already been written.
    pub fn patch_u8(&mut self, at: usize, v: u8) -> Result<(), Overflow> {
        if at >= self.pos {
            return Err(Overflow);
        }
        let slot = self.buf.get_mut(at).ok_or(Overflow)?;
        *slot = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_reads_and_fails_cleanly() {
        let data = [1u8, 0x34, 0x12, 2, 0xaa, 0xbb, 9];
        let mut r = Reader::new(&data);
        assert_eq!(r.u8(), Ok(1));
        assert_eq!(r.u16_le(), Ok(0x1234));
        assert_eq!(r.len8(), Ok(&[0xaa, 0xbb][..]));
        assert_eq!(r.remaining(), 1);
        assert_eq!(r.u16_le(), Err(Truncated));
        assert_eq!(r.array_back::<1>(), Ok(&[9]));
        assert!(r.is_empty());
        assert_eq!(r.u8(), Err(Truncated));
        let mut r = Reader::new(&[5, 1, 2]);
        assert_eq!(r.len8(), Err(Truncated));
    }

    #[test]
    fn writer_writes_patches_and_overflows() {
        let mut buf = [0u8; 4];
        let mut w = Writer::new(&mut buf);
        let at = w.reserve_u8().unwrap();
        w.put_u16_le(0xbeef).unwrap();
        w.patch_u8(at, 7).unwrap();
        assert_eq!(w.written(), &[7, 0xef, 0xbe]);
        assert_eq!(w.put_u16_le(1), Err(Overflow));
        assert_eq!(w.patch_u8(3, 1), Err(Overflow));
        w.put_u8(1).unwrap();
        assert_eq!(w.put_u8(1), Err(Overflow));
    }
}
