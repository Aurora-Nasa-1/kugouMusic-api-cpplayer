// Wire serialization & deserialization matching CPPlayer provider-sdk Wire.kt
#![allow(dead_code)]

pub struct WireWriter {
    buf: Vec<u8>,
}

impl Default for WireWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl WireWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512),
        }
    }

    pub fn write_byte(&mut self, val: u8) {
        self.buf.push(val);
    }

    pub fn write_int(&mut self, val: i32) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    pub fn write_long(&mut self, val: i64) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    pub fn write_bool(&mut self, val: bool) {
        self.buf.push(if val { 1 } else { 0 });
    }

    pub fn write_bytes(&mut self, val: &[u8]) {
        self.write_int(val.len() as i32);
        self.buf.extend_from_slice(val);
    }

    pub fn write_string(&mut self, val: &str) {
        self.write_bytes(val.as_bytes());
    }

    pub fn write_optional<T, F>(&mut self, val: &Option<T>, mut f: F)
    where
        F: FnMut(&mut Self, &T),
    {
        match val {
            Some(v) => {
                self.write_bool(true);
                f(self, v);
            }
            None => {
                self.write_bool(false);
            }
        }
    }

    pub fn write_list<T, F>(&mut self, val: &[T], mut f: F)
    where
        F: FnMut(&mut Self, &T),
    {
        self.write_int(val.len() as i32);
        for item in val {
            f(self, item);
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct WireReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> WireReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    pub fn read_byte(&mut self) -> Result<u8, String> {
        if self.offset >= self.data.len() {
            return Err("Truncated byte in wire".into());
        }
        let b = self.data[self.offset];
        self.offset += 1;
        Ok(b)
    }

    pub fn read_int(&mut self) -> Result<i32, String> {
        if self.offset + 4 > self.data.len() {
            return Err("Truncated int in wire".into());
        }
        let bytes: [u8; 4] = self.data[self.offset..self.offset + 4].try_into().unwrap();
        self.offset += 4;
        Ok(i32::from_le_bytes(bytes))
    }

    pub fn read_long(&mut self) -> Result<i64, String> {
        if self.offset + 8 > self.data.len() {
            return Err("Truncated long in wire".into());
        }
        let bytes: [u8; 8] = self.data[self.offset..self.offset + 8].try_into().unwrap();
        self.offset += 8;
        Ok(i64::from_le_bytes(bytes))
    }

    pub fn read_bool(&mut self) -> Result<bool, String> {
        match self.read_byte()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(format!("Invalid wire boolean: {}", b)),
        }
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, String> {
        let len = self.read_int()?;
        if len < 0 {
            return Err("Negative bytes length in wire".into());
        }
        let len = len as usize;
        if self.offset + len > self.data.len() {
            return Err("Truncated bytes in wire".into());
        }
        let bytes = self.data[self.offset..self.offset + len].to_vec();
        self.offset += len;
        Ok(bytes)
    }

    pub fn read_string(&mut self) -> Result<String, String> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 in wire string: {}", e))
    }

    pub fn read_optional<T, F>(&mut self, mut f: F) -> Result<Option<T>, String>
    where
        F: FnMut(&mut Self) -> Result<T, String>,
    {
        if self.read_bool()? {
            Ok(Some(f(self)?))
        } else {
            Ok(None)
        }
    }

    pub fn read_list<T, F>(&mut self, mut f: F) -> Result<Vec<T>, String>
    where
        F: FnMut(&mut Self) -> Result<T, String>,
    {
        let count = self.read_int()?;
        if count < 0 || count > 500 {
            return Err(format!("Invalid wire list count: {}", count));
        }
        let count = count as usize;
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            list.push(f(self)?);
        }
        Ok(list)
    }

    pub fn finish(&self) -> Result<(), String> {
        if self.offset == self.data.len() {
            Ok(())
        } else {
            Err(format!(
                "Trailing wire data: {} remaining",
                self.data.len() - self.offset
            ))
        }
    }
}
