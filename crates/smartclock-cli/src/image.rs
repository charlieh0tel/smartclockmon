//! A flash image read as the receivers' CPU32 reads it.

/// A firmware image, loaded at address zero: a stored pointer is a file
/// offset as it stands.
pub(crate) struct Image<'a>(pub(crate) &'a [u8]);

impl Image<'_> {
    /// The big-endian word at `at`.
    pub(crate) fn u16(&self, at: u32) -> Option<u16> {
        let at = usize::try_from(at).ok()?;
        Some(u16::from_be_bytes(self.0.get(at..at + 2)?.try_into().ok()?))
    }

    /// The big-endian long at `at`.
    pub(crate) fn u32(&self, at: u32) -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        Some(u32::from_be_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }

    /// Whether `at` is inside the image.
    pub(crate) fn holds(&self, at: u32) -> bool {
        usize::try_from(at).is_ok_and(|at| at < self.0.len())
    }
}
