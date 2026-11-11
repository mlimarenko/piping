//! Bounded byte console with streaming redaction before persistence.
use piping_contracts::playbook_runner::MAX_CONSOLE_CHUNK_BYTES;
use std::collections::VecDeque;

pub(crate) use piping_contracts::playbook_runner::{
    ConsoleChunk as Chunk, ConsoleStream as Stream,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConsoleError {
    EmptySecret,
    OutputLimit,
    ClosedStream,
}

/// One collector serializes reads from both streams. Only returned chunks may be
/// persisted. The caller must stop the job on OutputLimit and discard pending
/// output; known-secret redaction cannot protect unknown or transformed secrets.
#[derive(Debug)]
pub(crate) struct Collector {
    secrets: Vec<Vec<u8>>,
    pending: VecDeque<(Stream, u8)>,
    bytes_seen: usize,
    bytes_emitted: usize,
    chunks_emitted: usize,
    max_bytes: usize,
    max_chunks: usize,
    stdout_closed: bool,
    stderr_closed: bool,
    failed: bool,
}

impl Collector {
    pub(crate) fn new(
        secrets: Vec<Vec<u8>>,
        max_bytes: usize,
        max_chunks: usize,
    ) -> Result<Self, ConsoleError> {
        if secrets.iter().any(Vec::is_empty) {
            return Err(ConsoleError::EmptySecret);
        }
        Ok(Self {
            secrets,
            pending: VecDeque::new(),
            bytes_seen: 0,
            bytes_emitted: 0,
            chunks_emitted: 0,
            max_bytes,
            max_chunks,
            stdout_closed: false,
            stderr_closed: false,
            failed: false,
        })
    }

    pub(crate) fn push(
        &mut self,
        stream: Stream,
        bytes: &[u8],
    ) -> Result<Vec<Chunk>, ConsoleError> {
        if self.failed {
            return Err(ConsoleError::OutputLimit);
        }
        if match stream {
            Stream::Stdout => self.stdout_closed,
            Stream::Stderr => self.stderr_closed,
        } {
            return Err(ConsoleError::ClosedStream);
        }
        self.bytes_seen = match self.bytes_seen.checked_add(bytes.len()) {
            Some(size) if size <= self.max_bytes => size,
            _ => {
                self.failed = true;
                self.pending.clear();
                return Err(ConsoleError::OutputLimit);
            }
        };
        self.pending
            .extend(bytes.iter().map(|byte| (stream, *byte)));
        self.drain(false)
    }

    pub(crate) fn eof(&mut self, stream: Stream) -> Result<Vec<Chunk>, ConsoleError> {
        if self.failed {
            return Err(ConsoleError::OutputLimit);
        }
        let closed = match stream {
            Stream::Stdout => &mut self.stdout_closed,
            Stream::Stderr => &mut self.stderr_closed,
        };
        if *closed {
            return Err(ConsoleError::ClosedStream);
        }
        *closed = true;
        self.drain(self.stdout_closed && self.stderr_closed)
    }

    fn drain(&mut self, flush: bool) -> Result<Vec<Chunk>, ConsoleError> {
        let mut pieces: Vec<(Stream, Vec<u8>)> = Vec::new();
        while !self.pending.is_empty() {
            let stream = self.pending.front().expect("pending is not empty").0;
            let stream_closed = match stream {
                Stream::Stdout => self.stdout_closed,
                Stream::Stderr => self.stderr_closed,
            };
            let mut partial = false;
            let mut matched = Vec::new();
            for secret in &self.secrets {
                if self
                    .pending
                    .front()
                    .is_none_or(|(_, byte)| *byte != secret[0])
                {
                    continue;
                }
                for same_stream in [false, true] {
                    let mut candidates = self
                        .pending
                        .iter()
                        .enumerate()
                        .filter(|(_, (source, _))| !same_stream || *source == stream);
                    let mut positions = Vec::new();
                    let mut mismatch = false;
                    for expected in secret {
                        match candidates.next() {
                            Some((index, (_, actual))) if actual == expected => {
                                positions.push(index)
                            }
                            Some(_) => {
                                mismatch = true;
                                break;
                            }
                            None => break,
                        }
                    }
                    let compared = positions.len();
                    if !mismatch {
                        if compared == secret.len() && compared > matched.len() {
                            matched = positions;
                        } else if compared < secret.len()
                            && !(flush || same_stream && stream_closed)
                        {
                            partial = true;
                        }
                    }
                }
            }
            // Wait for a longer overlapping secret before choosing a shorter match.
            if partial {
                break;
            }
            let bytes = if !matched.is_empty() {
                for index in matched.into_iter().rev() {
                    self.pending.remove(index);
                }
                b"[REDACTED]".to_vec()
            } else {
                vec![self.pending.pop_front().expect("pending is not empty").1]
            };
            if let Some((last_stream, last_bytes)) =
                pieces.last_mut().filter(|(last_stream, last_bytes)| {
                    *last_stream == stream
                        && last_bytes.len() + bytes.len() <= MAX_CONSOLE_CHUNK_BYTES
                })
            {
                let _ = last_stream;
                last_bytes.extend(bytes);
            } else {
                pieces.push((stream, bytes));
            }
        }
        let output_bytes = pieces
            .iter()
            .try_fold(0usize, |sum, (_, bytes)| sum.checked_add(bytes.len()));
        if output_bytes
            .and_then(|size| self.bytes_emitted.checked_add(size))
            .is_none_or(|size| size > self.max_bytes)
            || self
                .chunks_emitted
                .checked_add(pieces.len())
                .is_none_or(|count| count > self.max_chunks)
        {
            self.failed = true;
            self.pending.clear();
            return Err(ConsoleError::OutputLimit);
        }
        self.bytes_emitted += output_bytes.expect("bounded above");
        Ok(pieces
            .into_iter()
            .map(|(stream, bytes)| {
                let sequence = self.chunks_emitted as u64;
                self.chunks_emitted += 1;
                Chunk {
                    sequence,
                    stream,
                    bytes,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_chunk_bound_preserves_large_binary_output() {
        let bytes: Vec<_> = (0..MAX_CONSOLE_CHUNK_BYTES * 2 + 3)
            .map(|index| (index % 256) as u8)
            .collect();
        let mut collector = Collector::new(vec![], bytes.len(), 8).unwrap();
        let chunks = collector.push(Stream::Stdout, &bytes).unwrap();
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.bytes.len() <= MAX_CONSOLE_CHUNK_BYTES)
        );
        assert_eq!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.bytes.iter().copied())
                .collect::<Vec<_>>(),
            bytes
        );
        let batch = piping_contracts::playbook_runner::RunnerConsoleBatch {
            dispatch_id: uuid::Uuid::new_v4(),
            dispatch_attempt: 1,
            chunks,
        };
        assert!(piping_contracts::playbook_runner::validate_console_batch(&batch).is_ok());
    }

    #[test]
    fn redacts_split_bytes_and_preserves_ansi_and_binary() {
        let mut collector = Collector::new(vec!["séc".as_bytes().to_vec()], 100, 20).unwrap();
        let mut out = collector.push(Stream::Stdout, b"\x1b[31ms\xc3").unwrap();
        out.extend(collector.push(Stream::Stdout, b"\xa9c\xff\x1b[0m").unwrap());
        out.extend(collector.eof(Stream::Stdout).unwrap());
        out.extend(collector.eof(Stream::Stderr).unwrap());
        assert_eq!(
            out.iter()
                .flat_map(|chunk| chunk.bytes.iter().copied())
                .collect::<Vec<_>>(),
            b"\x1b[31m[REDACTED]\xff\x1b[0m"
        );
        assert!(
            out.iter()
                .enumerate()
                .all(|(i, chunk)| chunk.sequence == i as u64)
        );
    }
    #[test]
    fn redacts_across_streams_and_flushes_partial_at_eof() {
        let mut collector = Collector::new(vec![b"secret".to_vec()], 100, 10).unwrap();
        let mut out = collector.push(Stream::Stdout, b"sec").unwrap();
        out.extend(collector.push(Stream::Stderr, b"ret!").unwrap());
        out.extend(collector.push(Stream::Stderr, b"se").unwrap());
        out.extend(collector.eof(Stream::Stdout).unwrap());
        out.extend(collector.eof(Stream::Stderr).unwrap());
        assert_eq!(
            out.iter()
                .flat_map(|chunk| chunk.bytes.iter().copied())
                .collect::<Vec<_>>(),
            b"[REDACTED]!se"
        );
        assert_eq!(out[0].stream, Stream::Stdout);
        assert!(out.iter().any(|chunk| chunk.stream == Stream::Stderr));
    }
    #[test]
    fn fails_closed_on_limits() {
        let mut collector = Collector::new(vec![b"secret".to_vec()], 3, 2).unwrap();
        assert!(collector.push(Stream::Stdout, b"sec").unwrap().is_empty());
        assert_eq!(
            collector.push(Stream::Stdout, b"r"),
            Err(ConsoleError::OutputLimit)
        );
        assert_eq!(
            collector.eof(Stream::Stdout),
            Err(ConsoleError::OutputLimit)
        );
        let mut chunks = Collector::new(vec![], 10, 0).unwrap();
        assert_eq!(
            chunks.push(Stream::Stdout, b"a"),
            Err(ConsoleError::OutputLimit)
        );
    }

    #[test]
    fn redacts_one_stream_even_when_the_other_interleaves() {
        let mut collector = Collector::new(vec![b"secret".to_vec()], 100, 10).unwrap();
        let mut out = collector.push(Stream::Stdout, b"sec").unwrap();
        out.extend(collector.push(Stream::Stderr, b"noise").unwrap());
        out.extend(collector.push(Stream::Stdout, b"ret").unwrap());
        out.extend(collector.eof(Stream::Stdout).unwrap());
        out.extend(collector.eof(Stream::Stderr).unwrap());
        let stdout: Vec<_> = out
            .iter()
            .filter(|c| c.stream == Stream::Stdout)
            .flat_map(|c| c.bytes.iter().copied())
            .collect();
        assert_eq!(stdout, b"[REDACTED]");
    }
}
