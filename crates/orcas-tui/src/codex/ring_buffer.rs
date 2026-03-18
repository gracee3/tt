#[derive(Debug, Clone)]
pub struct PtyRingBuffer {
    storage: Vec<u8>,
    start: usize,
    len: usize,
}

impl PtyRingBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            storage: vec![0; capacity],
            start: 0,
            len: 0,
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.storage.len()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, bytes: &[u8]) {
        let capacity = self.capacity();
        if capacity == 0 || bytes.is_empty() {
            return;
        }

        if bytes.len() >= capacity {
            let tail = &bytes[bytes.len() - capacity..];
            self.storage.copy_from_slice(tail);
            self.start = 0;
            self.len = capacity;
            return;
        }

        for &byte in bytes {
            if self.len < capacity {
                let index = (self.start + self.len) % capacity;
                self.storage[index] = byte;
                self.len += 1;
            } else {
                self.storage[self.start] = byte;
                self.start = (self.start + 1) % capacity;
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        if self.len == 0 {
            return Vec::new();
        }

        let capacity = self.capacity();
        let first_len = self.len.min(capacity - self.start);
        let mut snapshot = Vec::with_capacity(self.len);
        snapshot.extend_from_slice(&self.storage[self.start..self.start + first_len]);
        if self.len > first_len {
            snapshot.extend_from_slice(&self.storage[..self.len - first_len]);
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::PtyRingBuffer;

    #[test]
    fn preserves_only_last_n_bytes() {
        let mut ring = PtyRingBuffer::new(5);
        ring.push(b"hello world");
        assert_eq!(ring.snapshot(), b"world");
    }

    #[test]
    fn overwrites_oldest_bytes_when_capacity_is_reached() {
        let mut ring = PtyRingBuffer::new(4);
        ring.push(b"ab");
        ring.push(b"cd");
        ring.push(b"ef");
        assert_eq!(ring.snapshot(), b"cdef");
    }

    #[test]
    fn never_reports_more_than_capacity() {
        let mut ring = PtyRingBuffer::new(3);
        ring.push(b"a");
        ring.push(b"bc");
        ring.push(b"defgh");
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.snapshot(), b"fgh");
    }

    #[test]
    fn zero_capacity_ring_stays_empty() {
        let mut ring = PtyRingBuffer::new(0);
        ring.push(b"ignored");
        assert!(ring.is_empty());
        assert!(ring.snapshot().is_empty());
    }
}
