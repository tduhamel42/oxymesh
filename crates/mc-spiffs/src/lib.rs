//! Read-only SPIFFS, as the MeshCore C++ firmware leaves it on the ESP32 (ADR-0006), and the
//! import of its data ([`cpp`]). Nothing here writes to the flash it reads.
//!
//! On-disk format of SPIFFS (`spiffs_nucleus.h`, P. Andersson, MIT) with ESP-IDF's
//! configuration (`components/spiffs`, the arduino-esp32 build): 256-byte pages, 4 KB blocks,
//! 16-bit ids, 32-byte names, 4-byte metadata, block magic with length, packed headers. Each
//! block starts with an object lookup page: one 16-bit object id per following page (`0xFFFF`
//! free, `0` deleted, top bit set for index pages). A file is an index header page (span 0:
//! size, name, then the first page numbers of its data), more index pages if needed, and data
//! pages (5-byte header, then 251 bytes).
#![no_std]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod cpp;
mod json;

/// Reads the flash the file system lives in, from its start.
pub trait Flash {
    /// Fills `buf` from `offset`; `false` on a read error or out of range.
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool;
}

impl Flash for &[u8] {
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        let start = offset as usize;
        match self.get(start..start + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        }
    }
}

/// A closure `(offset, buf) -> ok` reads too (the firmware's partition).
#[derive(Debug)]
pub struct FnFlash<F>(pub F);

impl<F: FnMut(u32, &mut [u8]) -> bool> Flash for FnFlash<F> {
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        (self.0)(offset, buf)
    }
}

pub const PAGE: u32 = 256;
pub const BLOCK: u32 = 4096;
const PAGES_PER_BLOCK: u32 = BLOCK / PAGE;
/// Object lookup pages per block: max(1, 16 × 2 / 256).
const LOOKUP_PAGES: u32 = 1;
/// Pages per block a lookup entry describes.
const LOOKUP_ENTRIES: u32 = PAGES_PER_BLOCK - LOOKUP_PAGES;
const NAME_LEN: usize = 32;
/// Page header: object id, span index, flags (packed).
const PAGE_HEADER: u32 = 5;
/// Index header: page header, 3 alignment bytes, size, type, name, metadata.
const IX_HEADER: u32 = PAGE_HEADER + 3 + 4 + 1 + NAME_LEN as u32 + 4;
/// Index page header: page header and alignment.
const IX_PAGE_HEADER: u32 = PAGE_HEADER + 3;
/// Data page numbers in the index header, and in each further index page.
const HDR_IX_LEN: u32 = (PAGE - IX_HEADER) / 2;
const IX_LEN: u32 = (PAGE - IX_PAGE_HEADER) / 2;
const DATA_PAGE: u32 = PAGE - PAGE_HEADER;

const ID_FREE: u16 = 0xFFFF;
const ID_DELETED: u16 = 0;
const ID_IX_FLAG: u16 = 0x8000;
/// Page flags, active low.
const FLAG_USED: u8 = 1 << 0;
const FLAG_FINAL: u8 = 1 << 1;
const FLAG_INDEX: u8 = 1 << 2;
const FLAG_IXDELE: u8 = 1 << 6;
const FLAG_DELET: u8 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The flash could not be read.
    Read,
    /// No SPIFFS here (no block magic).
    NotSpiffs,
    NotFound,
    /// A page the file needs is missing or inconsistent.
    Corrupt,
}

/// An open file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    id: u16,
    header: u16,
    pub size: u32,
}

#[derive(Debug)]
pub struct Spiffs<F> {
    flash: F,
    blocks: u32,
    /// The last index page found: (object id with flag, span, page).
    ix_cache: Option<(u16, u16, u16)>,
}

#[derive(Debug, Clone, Copy)]
struct PageHeader {
    id: u16,
    span: u16,
    flags: u8,
}

impl<F: Flash> Spiffs<F> {
    /// The file system over `size` bytes of flash; checks the first block's magic.
    pub fn new(flash: F, size: u32) -> Result<Self, Error> {
        let mut fs = Self {
            flash,
            blocks: size / BLOCK,
            ix_cache: None,
        };
        if fs.blocks == 0 || !fs.magic_ok(0)? {
            return Err(Error::NotSpiffs);
        }
        Ok(fs)
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        if self.flash.read(offset, buf) {
            Ok(())
        } else {
            Err(Error::Read)
        }
    }

    fn u16_at(&mut self, offset: u32) -> Result<u16, Error> {
        let mut b = [0u8; 2];
        self.read(offset, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    /// `0x20140529 ^ page size ^ (blocks − block)`, cut to 16 bits, before the erase count at
    /// the end of the block's last lookup page.
    fn magic_ok(&mut self, block: u32) -> Result<bool, Error> {
        let want = (0x2014_0529 ^ PAGE ^ (self.blocks - block)) as u16;
        let at = block * BLOCK + LOOKUP_PAGES * PAGE - 4;
        Ok(self.u16_at(at)? == want)
    }

    fn header(&mut self, page: u32) -> Result<PageHeader, Error> {
        let mut b = [0u8; PAGE_HEADER as usize];
        self.read(page * PAGE, &mut b)?;
        let [i0, i1, s0, s1, flags] = b;
        Ok(PageHeader {
            id: u16::from_le_bytes([i0, i1]),
            span: u16::from_le_bytes([s0, s1]),
            flags,
        })
    }

    /// Every page in use with an id matching `pick`, block by block: `(page, id)`.
    fn scan(
        &mut self,
        mut visit: impl FnMut(&mut Self, u32, u16) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let mut lut = [0u8; (LOOKUP_ENTRIES * 2) as usize];
        for block in 0..self.blocks {
            self.read(block * BLOCK, &mut lut)?;
            for (e, pair) in lut.as_chunks::<2>().0.iter().enumerate() {
                let id = u16::from_le_bytes(*pair);
                if id == ID_FREE || id == ID_DELETED {
                    continue;
                }
                let page = block * PAGES_PER_BLOCK + LOOKUP_PAGES + e as u32;
                if visit(self, page, id)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// An index header (span 0, final, not deleted) and its name.
    fn index_header(
        &mut self,
        page: u32,
        id: u16,
    ) -> Result<Option<(File, [u8; NAME_LEN])>, Error> {
        if id & ID_IX_FLAG == 0 {
            return Ok(None);
        }
        let mut b = [0u8; IX_HEADER as usize];
        self.read(page * PAGE, &mut b)?;
        let span = u16::from_le_bytes([b[2], b[3]]);
        let flags = b[4];
        let live = flags & (FLAG_DELET | FLAG_FINAL | FLAG_IXDELE) == FLAG_DELET | FLAG_IXDELE;
        if span != 0 || !live {
            return Ok(None);
        }
        let size = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&b[13..13 + NAME_LEN]);
        let file = File {
            id,
            header: page as u16,
            // Never-written size: empty.
            size: if size == u32::MAX { 0 } else { size },
        };
        Ok(Some((file, name)))
    }

    /// The file named `name` (with its leading `/`).
    pub fn open(&mut self, name: &str) -> Result<File, Error> {
        let mut found = None;
        self.scan(|fs, page, id| {
            if let Some((f, n)) = fs.index_header(page, id)? {
                let len = n.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
                if n.get(..len) == Some(name.as_bytes()) {
                    found = Some(f);
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        found.ok_or(Error::NotFound)
    }

    /// Every file: name (up to the NUL) and size.
    pub fn list(&mut self, mut visit: impl FnMut(&[u8], u32)) -> Result<(), Error> {
        self.scan(|fs, page, id| {
            if let Some((f, n)) = fs.index_header(page, id)? {
                let len = n.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
                visit(n.get(..len).unwrap_or(&[]), f.size);
            }
            Ok(false)
        })
    }

    /// The index page of span `span` (≥ 1) of an object (`spiffs_obj_lu_find_id_and_span`).
    fn index_page(&mut self, ix_id: u16, span: u16) -> Result<u32, Error> {
        if let Some((i, s, p)) = self.ix_cache
            && i == ix_id
            && s == span
        {
            return Ok(u32::from(p));
        }
        let mut found = None;
        self.scan(|fs, page, id| {
            if id != ix_id {
                return Ok(false);
            }
            let h = fs.header(page)?;
            let live = h.flags & (FLAG_FINAL | FLAG_DELET | FLAG_USED) == FLAG_DELET
                && h.flags & FLAG_INDEX == 0;
            if h.id == ix_id && h.span == span && live {
                found = Some(page);
                return Ok(true);
            }
            Ok(false)
        })?;
        let page = found.ok_or(Error::Corrupt)?;
        self.ix_cache = Some((ix_id, span, page as u16));
        Ok(page)
    }

    /// The page holding data span `span` of `f`.
    fn data_page(&mut self, f: &File, span: u32) -> Result<u32, Error> {
        let at = if span < HDR_IX_LEN {
            u32::from(f.header) * PAGE + IX_HEADER + span * 2
        } else {
            let rest = span - HDR_IX_LEN;
            let ix_span = u16::try_from(1 + rest / IX_LEN).map_err(|_| Error::Corrupt)?;
            let page = self.index_page(f.id, ix_span)?;
            page * PAGE + IX_PAGE_HEADER + (rest % IX_LEN) * 2
        };
        let page = u32::from(self.u16_at(at)?);
        if page >= self.blocks * PAGES_PER_BLOCK || page % PAGES_PER_BLOCK < LOOKUP_PAGES {
            return Err(Error::Corrupt);
        }
        let h = self.header(page)?;
        if h.id != f.id & !ID_IX_FLAG || u32::from(h.span) != span {
            return Err(Error::Corrupt);
        }
        Ok(page)
    }

    /// Reads from `offset` into `buf`; the number of bytes read (fewer at the end of the file).
    pub fn read_at(&mut self, f: &File, offset: u32, buf: &mut [u8]) -> Result<usize, Error> {
        let mut done = 0usize;
        let mut pos = offset;
        while done < buf.len() && pos < f.size {
            let span = pos / DATA_PAGE;
            let within = pos % DATA_PAGE;
            let page = self.data_page(f, span)?;
            let n = (DATA_PAGE - within)
                .min(f.size - pos)
                .min((buf.len() - done) as u32) as usize;
            let dst = buf.get_mut(done..done + n).ok_or(Error::Corrupt)?;
            self.read(page * PAGE + PAGE_HEADER + within, dst)?;
            done += n;
            pos += n as u32;
        }
        Ok(done)
    }

    /// The whole file into `buf`; `Corrupt` if it does not fit.
    pub fn read_all<'b>(&mut self, name: &str, buf: &'b mut [u8]) -> Result<&'b [u8], Error> {
        let f = self.open(name)?;
        if f.size as usize > buf.len() {
            return Err(Error::Corrupt);
        }
        let n = self.read_at(&f, 0, buf)?;
        buf.get(..n).ok_or(Error::Corrupt)
    }
}
