use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::protocol::MAX_IPC_LINE_BYTES;

pub fn encode(value: &impl serde::Serialize) -> Result<String> {
    let mut s =
        serde_json::to_string(value).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

pub fn decode<T: serde::de::DeserializeOwned>(line: &str) -> Result<T> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(ObsctlError::IpcProtocolError("empty IPC frame".to_string()));
    }
    serde_json::from_str(trimmed).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))
}

/// Why a value could not be turned into a frame the other side will accept.
pub(crate) enum FrameEncodeError {
    /// The value itself could not be serialized.
    Encode(ObsctlError),
    /// The frame is longer than one line is allowed to be. `len` is its size
    /// in bytes, for the caller's log line.
    Oversized { len: usize },
}

/// Encode one value as a frame, refusing frames the reader would never accept.
///
/// The size rule belongs to both peers: each side reads at most
/// `MAX_IPC_LINE_BYTES` per line, so writing a longer one means the connection
/// is dropped rather than an error anyone can report. Checking it here, at the
/// only place that produces frames, is what stops the two ends from drifting
/// apart on where the limit sits.
pub(crate) fn encode_framed(
    value: &impl serde::Serialize,
) -> std::result::Result<String, FrameEncodeError> {
    let encoded = encode(value).map_err(FrameEncodeError::Encode)?;
    if encoded.len() > MAX_IPC_LINE_BYTES {
        return Err(FrameEncodeError::Oversized { len: encoded.len() });
    }
    Ok(encoded)
}

/// Why a frame could not be read.
pub(crate) enum FrameError {
    /// The peer sent more than `MAX_IPC_LINE_BYTES` without a newline.
    Oversized,
    /// The stream stopped part-way through a frame.
    MissingDelimiter,
    /// The completed frame was not valid UTF-8.
    NotUtf8(std::string::FromUtf8Error),
    /// The socket itself failed.
    Io(std::io::Error),
}

/// Reads newline-delimited frames from one connection, one frame at a time.
///
/// The bytes of a frame that has not arrived in full live in this struct
/// rather than inside the future returned by [`FrameReader::next_frame`].
/// That is the whole point of the type: the server reads inside a `select!`,
/// so whenever another branch wins the race the reading future is dropped
/// mid-frame. Anything the future was holding would be lost with it, and the
/// remainder of the frame would then be parsed as if it were a whole one.
/// Kept here instead, a half-read frame survives the cancellation and the
/// next call carries on where the last left off.
pub(crate) struct FrameReader<R: AsyncBufRead + Unpin> {
    reader: R,
    // Bytes, not a `String`: a frame is only known to be UTF-8 once it is
    // complete, and it may arrive in pieces.
    partial: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            partial: Vec::new(),
        }
    }

    /// Whether bytes of an unfinished frame are being held.
    ///
    /// This is what tells a failure apart from an ending: a socket that dies
    /// between frames is a peer that hung up, while one that dies with half a
    /// frame buffered has left a frame that will never be completed.
    pub fn has_partial_frame(&self) -> bool {
        !self.partial.is_empty()
    }

    /// Read the next frame, newline included.
    ///
    /// `Ok(None)` means the peer closed the connection between frames — the
    /// ordinary end of a session, not a failure. Cancelling this future (by
    /// dropping it, which `select!` does) keeps whatever was read so far.
    pub async fn next_frame(&mut self) -> std::result::Result<Option<String>, FrameError> {
        // How much more of the current frame may be read. The cap is applied
        // *while* reading rather than after: `read_line` would happily grow its
        // buffer to whatever a client sent before anyone got to check the size.
        // One byte over the limit is enough to recognise an oversized frame.
        let Some(budget) = (MAX_IPC_LINE_BYTES as u64 + 1).checked_sub(self.partial.len() as u64)
        else {
            return Err(FrameError::Oversized);
        };
        if budget == 0 {
            return Err(FrameError::Oversized);
        }

        // Bound this read to the frame's remaining budget. Recreated on every
        // call so a resumed frame keeps the bytes already buffered and the
        // limit still applies to the frame as a whole.
        let mut limited = (&mut self.reader).take(budget);

        // `read_until` rather than `read_line` because callers sit in a
        // `select!`: when another branch wins the race this future is dropped
        // mid-frame. `read_until` keeps whatever it had read in the buffer it
        // was given and can be resumed; `read_line` is documented as not
        // cancellation safe and discards it, which silently truncated any
        // frame unlucky enough to be split across a broadcast.
        match limited.read_until(b'\n', &mut self.partial).await {
            Ok(0) => Ok(None),
            Ok(_) => {
                if !self.partial.ends_with(b"\n") {
                    // No delimiter and the read stopped: either the budget ran
                    // out (oversized) or the peer went away mid-frame
                    // (truncated). Both end the session.
                    return Err(if self.partial.len() > MAX_IPC_LINE_BYTES {
                        FrameError::Oversized
                    } else {
                        FrameError::MissingDelimiter
                    });
                }

                match String::from_utf8(std::mem::take(&mut self.partial)) {
                    Ok(frame) => Ok(Some(frame)),
                    Err(error) => Err(FrameError::NotUtf8(error)),
                }
            }
            Err(error) => Err(FrameError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        value: String,
    }

    fn frames_of(bytes: &'static [u8]) -> FrameReader<&'static [u8]> {
        FrameReader::new(bytes)
    }

    #[test]
    fn round_trip() {
        let s = Sample {
            value: "hello".to_string(),
        };
        let encoded = encode(&s).unwrap();
        assert!(encoded.ends_with('\n'));
        let decoded: Sample = decode(&encoded).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn malformed_json_returns_error() {
        assert!(decode::<Sample>("not json").is_err());
    }

    #[test]
    fn empty_frame_is_error() {
        assert!(decode::<Sample>("").is_err());
        assert!(decode::<Sample>("\n").is_err());
        assert!(decode::<Sample>("\r\n").is_err());
        assert!(decode::<Sample>("   \n").is_err());
    }

    #[test]
    fn trim_newline_before_decode() {
        let value = decode::<Sample>("{\"value\":\"hello\"}\r\n").unwrap();
        assert_eq!(value.value, "hello");
    }

    #[test]
    fn encode_framed_accepts_a_frame_that_fits() {
        let encoded = encode_framed(&Sample {
            value: "hello".to_string(),
        })
        .ok()
        .expect("a small value fits in one frame");
        assert_eq!(encoded, "{\"value\":\"hello\"}\n");
    }

    #[test]
    fn encode_framed_rejects_a_frame_over_the_line_limit() {
        let sample = Sample {
            value: "x".repeat(MAX_IPC_LINE_BYTES),
        };
        match encode_framed(&sample) {
            Err(FrameEncodeError::Oversized { len }) => assert!(len > MAX_IPC_LINE_BYTES),
            _ => panic!("expected an oversized frame"),
        }
    }

    #[tokio::test]
    async fn next_frame_yields_each_line_then_none() {
        let mut frames = frames_of(b"{\"value\":\"a\"}\n{\"value\":\"b\"}\n");

        assert_eq!(
            frames.next_frame().await.ok().flatten(),
            Some("{\"value\":\"a\"}\n".to_string())
        );
        assert_eq!(
            frames.next_frame().await.ok().flatten(),
            Some("{\"value\":\"b\"}\n".to_string())
        );
        assert!(frames.next_frame().await.ok().flatten().is_none());
    }

    #[tokio::test]
    async fn next_frame_reports_a_truncated_frame() {
        let mut frames = frames_of(b"{\"value\":\"a\"}");

        assert!(matches!(
            frames.next_frame().await,
            Err(FrameError::MissingDelimiter)
        ));
    }

    #[tokio::test]
    async fn next_frame_reports_a_frame_past_the_line_limit() {
        // One long run of bytes with no delimiter anywhere.
        let flood: &'static [u8] = Box::leak(vec![b'a'; MAX_IPC_LINE_BYTES * 2].into_boxed_slice());
        let mut frames = FrameReader::new(flood);

        assert!(matches!(
            frames.next_frame().await,
            Err(FrameError::Oversized)
        ));
    }

    /// A source with no delimiter in it must be abandoned at the cap, not read
    /// until it happens to stop.
    ///
    /// The test above only proves the verdict, and it would reach the same
    /// verdict after swallowing the whole flood; what matters for memory is
    /// *when* the reading stops. `EndlessBytes` never ends and counts what it
    /// hands over, so an unbounded read shows up twice over: the byte count
    /// runs past the cap, and the source's own patience runs out and reports
    /// an I/O error instead of `Oversized`.
    #[tokio::test]
    async fn next_frame_stops_reading_a_line_that_never_ends() {
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::{Context, Poll};
        use tokio::io::{AsyncRead, BufReader, ReadBuf};

        /// Bytes with no newline in them, for as long as anyone keeps asking.
        struct EndlessBytes {
            served: Arc<AtomicUsize>,
            patience: usize,
        }

        impl AsyncRead for EndlessBytes {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.served.load(Ordering::Relaxed) >= self.patience {
                    return Poll::Ready(Err(std::io::Error::other("read far past the frame cap")));
                }
                const FILLER: [u8; 4096] = [b'a'; 4096];
                let n = buf.remaining().min(FILLER.len());
                buf.put_slice(&FILLER[..n]);
                self.served.fetch_add(n, Ordering::Relaxed);
                Poll::Ready(Ok(()))
            }
        }

        let served = Arc::new(AtomicUsize::new(0));
        let mut frames = FrameReader::new(BufReader::new(EndlessBytes {
            served: Arc::clone(&served),
            patience: MAX_IPC_LINE_BYTES * 8,
        }));

        assert!(matches!(
            frames.next_frame().await,
            Err(FrameError::Oversized)
        ));
        // The cap plus at most one read-ahead buffer; doubling it leaves room
        // for a different buffer size without leaving room for a runaway read.
        assert!(
            served.load(Ordering::Relaxed) < MAX_IPC_LINE_BYTES * 2,
            "read {} bytes of a line capped at {MAX_IPC_LINE_BYTES} bytes",
            served.load(Ordering::Relaxed)
        );
    }

    #[tokio::test]
    async fn next_frame_reports_a_frame_that_is_not_utf8() {
        let mut frames = frames_of(b"\xff\xfe\n");

        assert!(matches!(
            frames.next_frame().await,
            Err(FrameError::NotUtf8(_))
        ));
    }

    #[tokio::test]
    async fn a_cancelled_read_keeps_the_bytes_it_already_had() {
        use tokio::io::BufReader;

        let (client, server) = tokio::io::duplex(64);
        let mut frames = FrameReader::new(BufReader::new(server));

        let mut client = client;
        tokio::io::AsyncWriteExt::write_all(&mut client, b"{\"value\":")
            .await
            .unwrap();

        // Stand in for a `select!` branch winning the race: the read future is
        // dropped before the rest of the frame arrives.
        {
            let read = frames.next_frame();
            tokio::pin!(read);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(50), &mut read)
                    .await
                    .is_err(),
                "the frame is incomplete, so this read cannot finish"
            );
        }

        tokio::io::AsyncWriteExt::write_all(&mut client, b"\"hello\"}\n")
            .await
            .unwrap();

        let frame = frames.next_frame().await.ok().flatten().unwrap();
        assert_eq!(frame, "{\"value\":\"hello\"}\n");
        let decoded: Sample = decode(&frame).unwrap();
        assert_eq!(decoded.value, "hello");
    }
}
