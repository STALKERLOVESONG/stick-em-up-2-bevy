//! Little-endian binary encoding for the messages (`BinaryWriter` style: fixed-size numbers,
//! length-prefixed strings and lists).

use std::fmt;

/// A message could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed message: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

/// Lists and strings longer than this are rejected (a corrupt length must not allocate gigabytes).
const MAX_LENGTH: usize = 1 << 20;

#[derive(Default)]
pub struct Writer {
    pub bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }

    pub fn str(&mut self, s: &str) {
        self.len(s.len());
        self.bytes.extend_from_slice(s.as_bytes());
    }

    pub fn vec2(&mut self, v: [f32; 2]) {
        self.f32(v[0]);
        self.f32(v[1]);
    }

    pub fn option<T>(&mut self, v: &Option<T>, mut write: impl FnMut(&mut Writer, &T)) {
        self.bool(v.is_some());
        if let Some(v) = v {
            write(self, v);
        }
    }

    pub fn list<T>(&mut self, items: &[T], mut write: impl FnMut(&mut Writer, &T)) {
        self.len(items.len());
        for item in items {
            write(self, item);
        }
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or(DecodeError("truncated"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError("bad bool")),
        }
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    pub fn len(&mut self) -> Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n > MAX_LENGTH {
            return Err(DecodeError("length too large"));
        }
        Ok(n)
    }

    pub fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DecodeError("bad utf-8"))
    }

    pub fn vec2(&mut self) -> Result<[f32; 2], DecodeError> {
        Ok([self.f32()?, self.f32()?])
    }

    pub fn option<T>(&mut self, read: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>) -> Result<Option<T>, DecodeError> {
        if self.bool()? { Ok(Some(read(self)?)) } else { Ok(None) }
    }

    pub fn list<T>(&mut self, mut read: impl FnMut(&mut Reader<'a>) -> Result<T, DecodeError>) -> Result<Vec<T>, DecodeError> {
        let n = self.len()?;
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(read(self)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let mut w = Writer::new();
        w.u8(7);
        w.bool(true);
        w.i32(-5);
        w.f32(1.5);
        w.str("Player 2");
        w.option(&Some(3u32), |w, v| w.u32(*v));
        w.option(&None::<u32>, |w, v| w.u32(*v));
        w.list(&[1.0f32, 2.0], |w, v| w.f32(*v));
        let mut r = Reader::new(&w.bytes);
        assert_eq!(r.u8(), Ok(7));
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.i32(), Ok(-5));
        assert_eq!(r.f32(), Ok(1.5));
        assert_eq!(r.str().as_deref(), Ok("Player 2"));
        assert_eq!(r.option(|r| r.u32()), Ok(Some(3)));
        assert_eq!(r.option(|r| r.u32()), Ok(None));
        assert_eq!(r.list(|r| r.f32()), Ok(vec![1.0, 2.0]));
        assert!(r.is_empty());
    }

    #[test]
    fn truncated_and_oversized_input_is_rejected() {
        assert!(Reader::new(&[1, 2]).u32().is_err());
        let mut w = Writer::new();
        w.u32(u32::MAX);
        assert!(Reader::new(&w.bytes).str().is_err());
        assert!(Reader::new(&[2]).bool().is_err());
    }
}
