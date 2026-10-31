use std::io::Read;
use std::io::Write;

use crate::ReadingError;
use crate::WritingError;
use crate::ser::NetworkReadExt;
use crate::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BitSet(pub Box<[i64]>);

impl BitSet {
    const MAX_LENGTH: i32 = 1024;

    #[must_use]
    pub fn from_u64(val: u64) -> Self {
        Self(Box::new([val as i64]))
    }

    #[must_use]
    pub fn from_i64(val: i64) -> Self {
        Self(Box::new([val]))
    }

    #[must_use]
    pub fn from_longs(longs: Vec<i64>) -> Self {
        Self(longs.into_boxed_slice())
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.first().copied().unwrap_or(0) as u64
    }

    #[must_use]
    pub fn as_i64(&self) -> i64 {
        self.0.first().copied().unwrap_or(0)
    }

    #[must_use]
    pub fn get_bit(&self, index: usize) -> bool {
        let word_idx = index / 64;
        let bit_idx = index % 64;
        self.0
            .get(word_idx)
            .is_some_and(|&w| (w & (1i64 << bit_idx)) != 0)
    }

    pub fn set_bit(&mut self, index: usize, val: bool) {
        let word_idx = index / 64;
        let bit_idx = index % 64;
        if word_idx >= self.0.len() {
            let mut vec = self.0.to_vec();
            vec.resize(word_idx + 1, 0);
            self.0 = vec.into_boxed_slice();
        }
        if val {
            self.0[word_idx] |= 1i64 << bit_idx;
        } else {
            self.0[word_idx] &= !(1i64 << bit_idx);
        }
    }

    #[must_use]
    pub fn count_ones(&self) -> u32 {
        self.0.iter().map(|&w| (w as u64).count_ones()).sum()
    }

    fn checked_len(length: i32) -> Result<usize, ReadingError> {
        if !(0..=Self::MAX_LENGTH).contains(&length) {
            return Err(ReadingError::TooLarge("BitSet".to_string()));
        }
        Ok(length as usize)
    }

    pub fn encode(&self, write: &mut impl Write) -> Result<(), WritingError> {
        write.write_var_int(&self.0.len().try_into().map_err(|_| {
            WritingError::Message(format!("{} isn't representable as a VarInt", self.0.len()))
        })?)?;

        for b in &self.0 {
            write.write_i64_be(*b)?;
        }

        Ok(())
    }

    pub fn decode(read: &mut impl Read) -> Result<Self, ReadingError> {
        // Read length
        let length = Self::checked_len(read.get_var_int()?.0)?;
        let mut array: Vec<i64> = Vec::with_capacity(length);
        for _ in 0..length {
            let long = read.get_i64_be()?;
            array.push(long);
        }
        Ok(Self(array.into_boxed_slice()))
    }

    pub fn encode_with_version(
        &self,
        write: &mut impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        self.encode(write)
    }
    pub fn decode_with_version(
        read: &mut impl Read,
        _version: &JavaMinecraftVersion,
    ) -> Result<Self, ReadingError> {
        Self::decode(read)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_bitset_uses_big_endian_longs() {
        let bitset = BitSet::from_u64(1);
        let mut bytes = Vec::new();
        bitset
            .encode_with_version(&mut bytes, &JavaMinecraftVersion::V_26_2)
            .expect("encoding failed");
        assert_eq!(bytes, vec![1, 0, 0, 0, 0, 0, 0, 0, 1]);
        let decoded =
            BitSet::decode_with_version(&mut bytes.as_slice(), &JavaMinecraftVersion::V_26_2)
                .expect("decoding failed");
        assert!(decoded.get_bit(0));
    }
    #[test]
    fn rejects_negative_length() {
        let bytes = [0xff, 0xff, 0xff, 0xff, 0x0f];
        assert!(
            BitSet::decode_with_version(&mut bytes.as_slice(), &JavaMinecraftVersion::V_26_2)
                .is_err()
        );
    }
}
