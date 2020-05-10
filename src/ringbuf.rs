const TEST_BUFFER_SIZE: usize = 8192;

pub struct RingByteBuffer {
    buffer: [u8; TEST_BUFFER_SIZE],
    write: usize,
    read: usize,
}

impl RingByteBuffer {
    pub fn new() -> RingByteBuffer {
        RingByteBuffer {
            buffer: [0; TEST_BUFFER_SIZE],
            write: 0,
            read: 0,
        }
    }

    pub fn queue_size(&self) -> usize {
        if self.write >= self.read {
            self.write - self.read
        } else {
            self.buffer.len() - (self.read - self.write) - 1
        }
    }

    pub fn is_empty(&self) -> bool {
        self.write == self.read
    }

    pub fn is_full(&self) -> bool {
        (self.write + 1) % self.buffer.len() == self.read
    }

    pub fn get_free_slot(&mut self) -> &mut [u8] {
        if self.write >= self.read {
            &mut self.buffer[self.write..]
        } else {
            &mut self.buffer[self.write..self.read - 1]
        }
    }

    pub fn produce(&mut self, written: usize) {
        // FIXME check written validity
        self.write = (self.write + written) % self.buffer.len();
    }

    pub fn get_data(&self) -> &[u8] {
        if self.write >= self.read {
            &self.buffer[self.read..self.write]
        } else {
            &self.buffer[self.read..]
        }
    }

    pub fn consume(&mut self, read: usize) {
        // FIXME check read validity
        self.read = (self.read + read) % self.buffer.len();
    }
}

impl std::io::Write for RingByteBuffer {
    fn write(&mut self, src: &[u8]) -> std::io::Result<usize> {
        let mut written = 0;
        let mut src = src;
        while !src.is_empty() {
            let dst = self.get_free_slot();
            let write = if dst.len() < src.len() {
                dst.copy_from_slice(&src[..dst.len()]);
                dst.len()
            } else {
                dst[..src.len()].copy_from_slice(src);
                src.len()
            };

            self.produce(write);
            written += write;
            src = &src[write..];
        }

        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
