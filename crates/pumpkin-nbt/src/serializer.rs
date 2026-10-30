use std::io::Write;

use crate::Error;

/// Result type returned by NBT serialization operations.
pub type Result<T> = std::result::Result<T, Error>;

macro_rules! define_write_number_be {
    ($name:ident, $type:ty) => {
        fn $name(&mut self, value: $type) -> Result<()> {
            let buf = value.to_be_bytes();
            self.writer.write_all(&buf).map_err(Error::Incomplete)?;
            Ok(())
        }
    };
}

/// Format-specific primitive writer used by the NBT serializer.
pub trait NbtWriteHelper {
    /// Underlying output writer.
    type Writer: Write;

    /// Returns the underlying output writer.
    fn writer(&mut self) -> &mut Self::Writer;
    /// Writes an unsigned byte.
    fn write_u8(&mut self, value: u8) -> Result<()>;
    /// Writes a signed byte.
    fn write_i8(&mut self, value: i8) -> Result<()>;
    /// Writes a 16-bit signed integer.
    fn write_i16(&mut self, value: i16) -> Result<()>;
    /// Writes a 32-bit signed integer.
    fn write_i32(&mut self, value: i32) -> Result<()>;
    /// Writes a 64-bit signed integer.
    fn write_i64(&mut self, value: i64) -> Result<()>;
    /// Writes a 32-bit floating-point number.
    fn write_f32(&mut self, value: f32) -> Result<()>;
    /// Writes a 64-bit floating-point number.
    fn write_f64(&mut self, value: f64) -> Result<()>;
    /// Writes a length-prefixed string.
    fn write_string(&mut self, value: &str) -> Result<()>;

    /// Writes an unmodified byte slice.
    fn write_slice(&mut self, value: &[u8]) -> Result<()> {
        self.writer().write_all(value).map_err(Error::Incomplete)?;
        Ok(())
    }
}

/// Writes Java Edition NBT primitives using big-endian numeric encoding.
pub struct NbtWriteHelperJava<W: Write> {
    writer: W,
}

impl<W: Write> NbtWriteHelperJava<W> {
    /// Creates a Java Edition writer over `w`.
    pub const fn new(w: W) -> Self {
        Self { writer: w }
    }
}

impl<W: Write> NbtWriteHelperJava<W> {
    define_write_number_be!(write_string_len, u16);
}

impl<W: Write> NbtWriteHelper for NbtWriteHelperJava<W> {
    type Writer = W;

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    define_write_number_be!(write_u8, u8);
    define_write_number_be!(write_i8, i8);
    define_write_number_be!(write_i16, i16);
    define_write_number_be!(write_i32, i32);
    define_write_number_be!(write_i64, i64);
    define_write_number_be!(write_f32, f32);
    define_write_number_be!(write_f64, f64);

    fn write_string(&mut self, value: &str) -> Result<()> {
        let java_string = cesu8::to_java_cesu8(value);
        let len = java_string.len();
        if len > u16::MAX as usize {
            return Err(Error::LargeLength(len));
        }

        self.write_string_len(len as u16)?;
        self.writer
            .write_all(&java_string)
            .map_err(Error::Incomplete)?;
        Ok(())
    }
}
