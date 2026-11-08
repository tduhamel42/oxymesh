//! Two views of the one flash (ADR-0006): `mcdata`, read and written by the store, and the C++
//! firmware's SPIFFS, which can only be read: [`ReadOnly`] has no way to write or erase, so the
//! import cannot touch 0x290000–0x3EFFFF.
//!
//! Both share a `RefCell<FlashStorage>`; each call borrows it for the duration of one blocking
//! flash operation (nothing awaits inside), so they never overlap.

use core::cell::RefCell;

use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
use esp_storage::{FlashStorage, FlashStorageError};

/// A partition, read and written at offsets from its start.
pub struct Region<'a, 'd> {
    flash: &'a RefCell<FlashStorage<'d>>,
    offset: u32,
    len: u32,
}

impl<'a, 'd> Region<'a, 'd> {
    pub fn new(flash: &'a RefCell<FlashStorage<'d>>, offset: u32, len: u32) -> Self {
        Self { flash, offset, len }
    }

    fn check(&self, from: u32, n: usize) -> Result<u32, FlashStorageError> {
        let end = from
            .checked_add(n as u32)
            .ok_or(FlashStorageError::OutOfBounds)?;
        if end > self.len {
            return Err(FlashStorageError::OutOfBounds);
        }
        Ok(self.offset + from)
    }
}

impl ErrorType for Region<'_, '_> {
    type Error = FlashStorageError;
}

impl ReadNorFlash for Region<'_, '_> {
    const READ_SIZE: usize = <FlashStorage<'static> as ReadNorFlash>::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let at = self.check(offset, bytes.len())?;
        ReadNorFlash::read(&mut *self.flash.borrow_mut(), at, bytes)
    }

    fn capacity(&self) -> usize {
        self.len as usize
    }
}

impl NorFlash for Region<'_, '_> {
    const WRITE_SIZE: usize = <FlashStorage<'static> as NorFlash>::WRITE_SIZE;
    const ERASE_SIZE: usize = <FlashStorage<'static> as NorFlash>::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if to < from {
            return Err(FlashStorageError::OutOfBounds);
        }
        let start = self.check(from, (to - from) as usize)?;
        NorFlash::erase(&mut *self.flash.borrow_mut(), start, start + (to - from))
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let at = self.check(offset, bytes.len())?;
        NorFlash::write(&mut *self.flash.borrow_mut(), at, bytes)
    }
}

/// A partition that can only be read, at any offset and length.
pub struct ReadOnly<'a, 'd> {
    flash: &'a RefCell<FlashStorage<'d>>,
    offset: u32,
    len: u32,
}

impl<'a, 'd> ReadOnly<'a, 'd> {
    pub fn new(flash: &'a RefCell<FlashStorage<'d>>, offset: u32, len: u32) -> Self {
        Self { flash, offset, len }
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl mc_spiffs::Flash for ReadOnly<'_, '_> {
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        let Some(end) = offset.checked_add(buf.len() as u32) else {
            return false;
        };
        end <= self.len
            && self
                .flash
                .borrow_mut()
                .read(self.offset + offset, buf)
                .is_ok()
    }
}
