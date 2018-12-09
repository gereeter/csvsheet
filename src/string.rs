// TODO: There are much better encodings of small strings, but this is simple and none of the existing libraries implement all the desired functions (remove a range, in particular).
// This should be replaced by something else when those APIs get created

#[derive(Clone)]
pub struct SmallString {
    bytes: smallvec::SmallVec<[u8; 15]> // The maximum that results in the same size as String on 64bit
}

impl std::ops::Deref for SmallString {
    type Target = str;

    fn deref(&self) -> &str {
        unsafe {
            std::str::from_utf8_unchecked(&self.bytes)
        }
    }
}

impl std::ops::DerefMut for SmallString {
    fn deref_mut(&mut self) -> &mut str {
        unsafe {
            std::str::from_utf8_unchecked_mut(&mut self.bytes)
        }
    }
}

impl SmallString {
    pub fn new() -> SmallString {
        SmallString {
            bytes: smallvec::SmallVec::new()
        }
    }

    pub fn from_str(value: &str) -> SmallString {
        SmallString {
            bytes: smallvec::SmallVec::from_slice(value.as_bytes())
        }
    }

    pub fn insert(&mut self, index: usize, chr: char) {
        let mut chr_bytes = [0; 4];
        self.bytes.insert_from_slice(index, chr.encode_utf8(&mut chr_bytes).as_bytes());
    }

    pub fn remove_range(&mut self, range: std::ops::Range<usize>) {
        // Assert that the range is valid utf8 character boundaries
        assert!(self.is_char_boundary(range.start));
        assert!(self.is_char_boundary(range.end));

        let shift_len = range.end - range.start;
        for i in range.start..self.len() - shift_len {
            self.bytes[i] = self.bytes[i + shift_len];
        }
        self.bytes.truncate(self.len() - shift_len);
    }
}
