//! DVRIP / Sofia / NetSurveillance wire codec (Xiongmai family, TCP 34567).
//!
//! This is the pure, I/O-free layer: the Sofia password hash, the 20-byte frame
//! header, and the command identifiers. A driver layers a byte transport and
//! JSON (de)serialization on top of these primitives.
//!
//! The frame layout, verified against live devices and two reference
//! implementations, is a little-endian 20-byte header followed by a payload:
//!
//! ```text
//! offset  size  field
//!   0      1    head flag (0xFF)
//!   1      1    version (0x00)
//!   2      2    reserved
//!   4      4    session id (u32 LE)
//!   8      4    sequence  (u32 LE)
//!  12      2    reserved (channel/packet)
//!  14      2    message id (u16 LE)
//!  16      4    payload length (u32 LE)
//!  20      N    payload: UTF-8 JSON, NUL-terminated
//! ```

use md5::{Digest, Md5};
use thiserror::Error;

/// The 62-character alphabet the Sofia hash maps digest byte-pairs onto.
const SOFIA_ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Length of the fixed DVRIP frame header, in bytes.
pub const HEADER_LEN: usize = 20;

/// First byte of every DVRIP frame.
const HEAD_FLAG: u8 = 0xFF;

/// DVRIP command identifiers (the `message id` header field).
///
/// Login and the read commands are verified against live devices; the write
/// commands are taken from reference implementations and must not be issued
/// against a device outside an explicit, gated write path.
pub mod command {
    /// `LOGIN_REQ2` — authenticate and obtain a session id.
    pub const LOGIN: u16 = 1000;
    /// `KEEPALIVE` — keep the session alive at the login-provided interval.
    pub const KEEPALIVE: u16 = 1006;
    /// Named config/system query (`SystemInfo`, `StorageInfo`, …).
    pub const CONFIG_QUERY: u16 = 1020;
    /// `AuthorityList` — the device's authority-token catalog.
    pub const AUTHORITY_LIST: u16 = 1470;
    /// `Users` — list user accounts.
    pub const USERS: u16 = 1472;
    /// `AddUser` — create a user (write; gated).
    pub const ADD_USER: u16 = 1482;
    /// `ModifyUser` — modify a user (write; gated).
    pub const MODIFY_USER: u16 = 1484;
    /// `DelUser` — delete a user (write; gated).
    pub const DELETE_USER: u16 = 1486;
    /// `ModifyPassword` — change a user's password (write; gated).
    pub const MODIFY_PASSWORD: u16 = 1488;
    /// `OPSNAP` — capture a still frame.
    pub const SNAPSHOT: u16 = 1560;
    /// `OPPTZControl` — pan/tilt/zoom and preset control (write; gated).
    pub const PTZ: u16 = 1400;
    /// `OPFileQuery` — enumerate recorded files for one channel and window.
    ///
    /// A pure READ of stored footage metadata. Live-verified on three distinct
    /// firmwares: replies on `FILE_QUERY + 1` with `Ret = 100` and an
    /// `OPFileQuery` array of per-hour segments.
    pub const FILE_QUERY: u16 = 1440;
    /// `OPPlayBack` with `Action: "Claim"` — reserve an archive playback.
    ///
    /// Replies on `PLAYBACK_CLAIM + 1`. Note the asymmetry with
    /// [`PLAYBACK_START`]: the claim and the start use DIFFERENT message ids.
    pub const PLAYBACK_CLAIM: u16 = 1424;
    /// `OPPlayBack` with `Action: "DownloadStart"` — begin streaming the claimed
    /// window. The device does not answer this frame; it starts pushing media
    /// frames tagged [`PLAYBACK_MEDIA`] on the same socket.
    pub const PLAYBACK_START: u16 = 1420;
    /// Message id carrying archive media payload after a `DownloadStart`.
    pub const PLAYBACK_MEDIA: u16 = 1426;
}

/// The XM media-packet layer that rides inside [`command::PLAYBACK_MEDIA`]
/// frames.
///
/// Archive playback does not deliver a container: after the 20-byte DVRIP frame
/// headers are stripped, the concatenated bodies form a continuous stream of
/// XM packets, each wrapping a slice of an elementary video (or audio) stream.
/// One video frame is routinely split across several DVRIP frames — the device
/// emits fixed-size bodies — so a parser MUST operate on the joined stream, never
/// per frame.
///
/// Packet layout, live-verified across three distinct firmwares:
///
/// ```text
/// magic         packet     header  payload length
/// 00 00 01 FC   video I    16 B    u32 at offset +12
/// 00 00 01 FD   video P     8 B    u32 at offset +4
/// 00 00 01 FA   audio       8 B    u16 at offset +6
/// 00 00 01 F9   aux/info    8 B    u16 at offset +6
/// ```
///
/// The audio length being 16-bit is the trap in this format: reading it as a
/// `u32` desynchronizes the whole stream on any unit that records audio, which
/// silently discards a double-digit percentage of the video bytes.
pub mod media {
    /// Video I-frame packet; its header additionally carries fps and a timestamp.
    pub const KIND_VIDEO_I: u8 = 0xFC;
    /// Video P-frame packet.
    pub const KIND_VIDEO_P: u8 = 0xFD;
    /// Audio packet (dropped: the archive plane is video-only today).
    pub const KIND_AUDIO: u8 = 0xFA;
    /// Auxiliary/info packet (dropped).
    pub const KIND_AUX: u8 = 0xF9;
}

/// Compute the Sofia/DVRIP password hash used in the login payload.
///
/// The algorithm is: take the MD5 digest of the UTF-8 password; for each of the
/// first 8 byte-pairs, sum the pair modulo 62 and index `SOFIA_ALPHABET`. The
/// result is always 8 ASCII-alphanumeric characters. Note this is a weak,
/// fixed-length hash defined by the protocol, not a security primitive.
#[must_use]
pub fn sofia_hash(password: &str) -> String {
    let digest = Md5::digest(password.as_bytes());
    let mut out = String::with_capacity(8);
    for pair in 0..8 {
        let sum = u16::from(digest[2 * pair]) + u16::from(digest[2 * pair + 1]);
        let index = (sum % 62) as usize;
        out.push(char::from(SOFIA_ALPHABET[index]));
    }
    out
}

/// A decoded DVRIP frame (header fields plus the payload, terminators stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Session id from the header (0 before login completes).
    pub session: u32,
    /// Sequence number from the header.
    pub sequence: u32,
    /// Command identifier from the header.
    pub message_id: u16,
    /// Payload bytes with trailing NUL/newline terminators removed.
    pub payload: Vec<u8>,
}

/// Errors from decoding a DVRIP frame.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WireError {
    /// The buffer is shorter than the fixed header.
    #[error("frame shorter than the {HEADER_LEN}-byte header")]
    ShortHeader,
    /// The head flag byte was not `0xFF`.
    #[error("invalid head flag {0:#04x}")]
    BadHeadFlag(u8),
    /// The header declared more payload than the buffer contains.
    #[error("declared payload length {declared} exceeds available {available}")]
    TruncatedPayload {
        /// Length the header claimed.
        declared: usize,
        /// Bytes actually present after the header.
        available: usize,
    },
}

/// Encode a JSON payload into a DVRIP request frame.
///
/// The payload is the JSON text followed by the protocol's `\n\0` terminator;
/// the declared length includes those two bytes.
#[must_use]
pub fn encode(message_id: u16, session: u32, sequence: u32, json: &str) -> Vec<u8> {
    let payload_len = json.len() + 2;
    let mut buf = Vec::with_capacity(HEADER_LEN + payload_len);
    buf.push(HEAD_FLAG);
    buf.push(0x00); // version
    buf.extend_from_slice(&[0x00, 0x00]); // reserved
    buf.extend_from_slice(&session.to_le_bytes());
    buf.extend_from_slice(&sequence.to_le_bytes());
    buf.extend_from_slice(&[0x00, 0x00]); // reserved (channel/packet)
    buf.extend_from_slice(&message_id.to_le_bytes());
    buf.extend_from_slice(&(payload_len as u32).to_le_bytes());
    buf.extend_from_slice(json.as_bytes());
    buf.push(0x0A); // newline
    buf.push(0x00); // NUL terminator
    buf
}

/// Decode a complete DVRIP frame (header + payload) from `buf`.
///
/// Trailing NUL and newline terminators the device appends are stripped from
/// the returned payload.
///
/// # Errors
///
/// Returns [`WireError`] if the buffer is too short, the head flag is wrong, or
/// the declared payload length exceeds the buffer.
pub fn decode(buf: &[u8]) -> Result<Frame, WireError> {
    if buf.len() < HEADER_LEN {
        return Err(WireError::ShortHeader);
    }
    if buf[0] != HEAD_FLAG {
        return Err(WireError::BadHeadFlag(buf[0]));
    }
    let session = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let sequence = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let message_id = u16::from_le_bytes([buf[14], buf[15]]);
    let declared = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) as usize;
    let available = buf.len() - HEADER_LEN;
    if declared > available {
        return Err(WireError::TruncatedPayload {
            declared,
            available,
        });
    }
    let mut payload = buf[HEADER_LEN..HEADER_LEN + declared].to_vec();
    while matches!(payload.last(), Some(&0x00 | &0x0A)) {
        payload.pop();
    }
    Ok(Frame {
        session,
        sequence,
        message_id,
        payload,
    })
}

/// Playback hints read off the wire from a video I-frame packet header.
///
/// Only [`fps`](Self::fps) is trustworthy enough to drive the remux: the same
/// header's codec-id and geometry bytes proved unreliable in live capture (one
/// firmware reported `512x1440` for a genuine `2560x1440` HEVC stream, and the
/// codec id carries flags in its high nibble), so the container prober decides
/// codec and geometry instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaHints {
    /// Frames per second as the device reports them. Verified accurate on every
    /// captured unit, and needed because these recorders commonly run at 12 fps
    /// while a remux with no rate hint would assume 25 and play at double speed.
    pub fps: u8,
}

/// Incremental de-framer turning an XM media byte stream into elementary video.
///
/// Feed it the concatenated bodies of [`command::PLAYBACK_MEDIA`] frames; it
/// yields the video elementary stream with every XM wrapper removed, buffering
/// only the tail of a packet that has not fully arrived. Audio and auxiliary
/// packets are dropped — the archive plane is video-only.
///
/// This type is deliberately I/O-free so the framing rules are unit-testable
/// against captured byte vectors without touching a device.
#[derive(Debug, Default)]
pub struct MediaDeframer {
    pending: Vec<u8>,
    hints: Option<MediaHints>,
    dropped_bytes: usize,
}

impl MediaDeframer {
    /// A de-framer with an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hints from the first video I-frame header seen so far, if any.
    #[must_use]
    pub fn hints(&self) -> Option<MediaHints> {
        self.hints
    }

    /// Bytes discarded while resynchronizing on a packet boundary.
    ///
    /// Non-zero is normal exactly once, when a capture joins mid-packet; a value
    /// that keeps growing means the framing rules and the device disagree, which
    /// is worth surfacing rather than silently producing a corrupt stream.
    #[must_use]
    pub fn dropped_bytes(&self) -> usize {
        self.dropped_bytes
    }

    /// Consume `chunk`, returning any complete elementary-stream bytes it
    /// completed. Incomplete trailing packets are retained for the next call.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut pos = 0;

        while pos + MIN_MEDIA_PACKET_LEN <= self.pending.len() {
            let Some(kind) = packet_kind_at(&self.pending, pos) else {
                // Not on a packet boundary: skip to the next plausible magic.
                let Some(next) = find_magic(&self.pending, pos + 1) else {
                    self.dropped_bytes += self.pending.len() - pos;
                    pos = self.pending.len();
                    break;
                };
                self.dropped_bytes += next - pos;
                pos = next;
                continue;
            };
            let (header_len, length_offset, length_width) = packet_layout(kind);
            if pos + header_len > self.pending.len() {
                break;
            }
            let length = read_length(&self.pending, pos + length_offset, length_width);
            if length > MAX_MEDIA_PACKET_PAYLOAD {
                // A length this large cannot be real; treat the magic as a false
                // positive inside payload bytes and resynchronize past it.
                self.dropped_bytes += 3;
                pos += 3;
                continue;
            }
            if pos + header_len + length > self.pending.len() {
                break; // packet still arriving
            }
            if kind == media::KIND_VIDEO_I && self.hints.is_none() {
                self.hints = Some(MediaHints {
                    fps: self.pending[pos + 5],
                });
            }
            if matches!(kind, media::KIND_VIDEO_I | media::KIND_VIDEO_P) {
                out.extend_from_slice(&self.pending[pos + header_len..pos + header_len + length]);
            }
            pos += header_len + length;
        }

        self.pending.drain(..pos);
        out
    }
}

/// Shortest packet this layer can decide anything about (audio/aux header).
const MIN_MEDIA_PACKET_LEN: usize = 8;
/// Refuse a declared payload larger than this: real packets are far smaller, and
/// a device-controlled length must never drive an unbounded wait.
const MAX_MEDIA_PACKET_PAYLOAD: usize = 4 * 1024 * 1024;

/// The XM packet kind at `pos`, or `None` when `pos` is not a packet boundary.
fn packet_kind_at(buf: &[u8], pos: usize) -> Option<u8> {
    if buf.get(pos..pos + 3)? != [0x00, 0x00, 0x01] {
        return None;
    }
    let kind = *buf.get(pos + 3)?;
    matches!(
        kind,
        media::KIND_VIDEO_I | media::KIND_VIDEO_P | media::KIND_AUDIO | media::KIND_AUX
    )
    .then_some(kind)
}

/// `(header_len, length_offset, length_width)` for a known packet kind.
fn packet_layout(kind: u8) -> (usize, usize, usize) {
    match kind {
        media::KIND_VIDEO_I => (16, 12, 4),
        media::KIND_VIDEO_P => (8, 4, 4),
        // Audio and aux declare a 16-bit length; reading 32 bits here desyncs
        // the stream on every unit that records audio.
        _ => (8, 6, 2),
    }
}

/// Read a little-endian length of `width` bytes at `offset`.
fn read_length(buf: &[u8], offset: usize, width: usize) -> usize {
    if width == 2 {
        usize::from(u16::from_le_bytes([buf[offset], buf[offset + 1]]))
    } else {
        u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize
    }
}

/// Index of the next `00 00 01` magic at or after `from`.
fn find_magic(buf: &[u8], from: usize) -> Option<usize> {
    buf.get(from..)?
        .windows(3)
        .position(|window| window == [0x00, 0x00, 0x01])
        .map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one XM video I-frame packet around `payload`.
    fn video_i_packet(fps: u8, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x00, 0x00, 0x01, media::KIND_VIDEO_I];
        packet.extend_from_slice(&[0x03, fps, 116, 72]); // codec id, fps, w/8, h/8
        packet.extend_from_slice(&0x69F2_4000_u32.to_le_bytes()); // packed timestamp
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    /// Build one XM video P-frame packet around `payload`.
    fn video_p_packet(payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x00, 0x00, 0x01, media::KIND_VIDEO_P];
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    /// Build one XM audio packet — note the 16-bit length.
    fn audio_packet(payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x00, 0x00, 0x01, media::KIND_AUDIO];
        packet.extend_from_slice(&[0x0E, 0x02]); // codec/sample-rate descriptor
        packet.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    const NAL_ONE: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0C, 0x01];
    const NAL_TWO: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAF];

    #[test]
    fn deframer_strips_wrappers_and_keeps_only_video() {
        let mut stream = video_i_packet(12, NAL_ONE);
        stream.extend(audio_packet(&[0x11, 0x22, 0x33, 0x44]));
        stream.extend(video_p_packet(NAL_TWO));

        let mut deframer = MediaDeframer::new();
        let video = deframer.push(&stream);

        assert_eq!(video, [NAL_ONE, NAL_TWO].concat());
        assert_eq!(deframer.dropped_bytes(), 0);
    }

    /// The audio length is 16-bit. Reading it as 32-bit consumes far too many
    /// bytes and desynchronizes everything after it — the exact failure that
    /// silently discarded video on units that record audio.
    #[test]
    fn deframer_reads_a_sixteen_bit_audio_length() {
        let mut stream = audio_packet(&[0xAA; 6]);
        stream.extend(video_p_packet(NAL_TWO));

        let mut deframer = MediaDeframer::new();

        assert_eq!(deframer.push(&stream), NAL_TWO);
        assert_eq!(deframer.dropped_bytes(), 0);
    }

    /// The device splits one video frame across several transport frames, so the
    /// de-framer must join chunks rather than decode each in isolation.
    #[test]
    fn deframer_joins_a_packet_split_across_chunks() {
        let stream = video_i_packet(12, NAL_ONE);
        let (head, tail) = stream.split_at(stream.len() - 3);

        let mut deframer = MediaDeframer::new();

        assert!(deframer.push(head).is_empty(), "partial packet must buffer");
        assert_eq!(deframer.push(tail), NAL_ONE);
    }

    /// A capture that joins mid-packet must resynchronize on the next boundary
    /// instead of emitting garbage, and must account for what it skipped.
    #[test]
    fn deframer_resynchronizes_after_a_mid_packet_join() {
        let mut stream = vec![0xDE, 0xAD, 0xBE, 0xEF];
        stream.extend(video_p_packet(NAL_TWO));

        let mut deframer = MediaDeframer::new();

        assert_eq!(deframer.push(&stream), NAL_TWO);
        assert_eq!(deframer.dropped_bytes(), 4);
    }

    #[test]
    fn deframer_reports_fps_from_the_first_i_frame_header() {
        let mut deframer = MediaDeframer::new();
        assert_eq!(deframer.hints(), None);

        deframer.push(&video_i_packet(12, NAL_ONE));
        assert_eq!(deframer.hints(), Some(MediaHints { fps: 12 }));

        // A later I-frame must not overwrite the established rate.
        deframer.push(&video_i_packet(25, NAL_ONE));
        assert_eq!(deframer.hints(), Some(MediaHints { fps: 12 }));
    }

    /// A device-controlled length must never make the de-framer wait for an
    /// impossible packet; it resynchronizes instead of stalling forever.
    #[test]
    fn deframer_rejects_an_implausible_declared_length() {
        let mut stream = vec![0x00, 0x00, 0x01, media::KIND_VIDEO_P];
        stream.extend_from_slice(&u32::MAX.to_le_bytes());
        stream.extend(video_p_packet(NAL_TWO));

        let mut deframer = MediaDeframer::new();

        assert_eq!(deframer.push(&stream), NAL_TWO);
        assert!(deframer.dropped_bytes() > 0);
    }

    /// Payload bytes may contain `00 00 01` followed by a byte that is not a
    /// known packet kind — a real NAL start code. It must not be mistaken for a
    /// wrapper.
    #[test]
    fn deframer_ignores_nal_start_codes_inside_payloads() {
        let payload = [0x00, 0x00, 0x01, 0x65, 0x88, 0x99];
        let mut deframer = MediaDeframer::new();

        assert_eq!(deframer.push(&video_p_packet(&payload)), payload);
        assert_eq!(deframer.dropped_bytes(), 0);
    }

    /// Real packet HEADERS captured from live firmware (payloads are not
    /// included — only the framing bytes, so no footage is committed).
    ///
    /// Two firmwares are represented because they exercise different rules: the
    /// first records video only, the second interleaves audio, which is where a
    /// 32-bit audio-length read desynchronizes the stream.
    mod captured_headers {
        /// Video-only firmware: I-frame then P-frames.
        pub(super) const VIDEO_ONLY: &[(&[u8], usize)] = &[
            (
                &[
                    0x00, 0x00, 0x01, 0xFC, 0x03, 0x0C, 0x74, 0x48, 0x00, 0x50, 0xF2, 0x69, 0x39,
                    0xE1, 0x00, 0x00,
                ],
                57657,
            ),
            (&[0x00, 0x00, 0x01, 0xFD, 0xC5, 0x08, 0x00, 0x00], 2245),
            (&[0x00, 0x00, 0x01, 0xFD, 0x21, 0x07, 0x00, 0x00], 1825),
        ];

        /// Audio-carrying firmware: note the `FA` packets' 16-bit length (320).
        pub(super) const WITH_AUDIO: &[(&[u8], usize)] = &[
            (
                &[
                    0x00, 0x00, 0x01, 0xFC, 0x13, 0x14, 0x40, 0xB4, 0x00, 0x50, 0xF2, 0x69, 0x98,
                    0x55, 0x03, 0x00,
                ],
                218_520,
            ),
            (&[0x00, 0x00, 0x01, 0xFD, 0x85, 0x00, 0x00, 0x00], 133),
            (&[0x00, 0x00, 0x01, 0xFA, 0x0E, 0x02, 0x40, 0x01], 320),
            (&[0x00, 0x00, 0x01, 0xFA, 0x0E, 0x02, 0x40, 0x01], 320),
        ];
    }

    /// Rebuild a stream from captured headers, filling each payload with a
    /// byte pattern derived from its index so mis-attributed bytes are visible.
    fn stream_from_captured(packets: &[(&[u8], usize)]) -> (Vec<u8>, Vec<u8>) {
        let mut stream = Vec::new();
        let mut expected_video = Vec::new();
        for (index, (header, declared_len)) in packets.iter().enumerate() {
            let payload = vec![index as u8; *declared_len];
            stream.extend_from_slice(header);
            stream.extend_from_slice(&payload);
            if matches!(header[3], media::KIND_VIDEO_I | media::KIND_VIDEO_P) {
                expected_video.extend_from_slice(&payload);
            }
        }
        (stream, expected_video)
    }

    /// Captured video-only framing round-trips with no loss.
    #[test]
    fn deframer_handles_captured_video_only_framing() {
        let (stream, expected) = stream_from_captured(captured_headers::VIDEO_ONLY);

        let mut deframer = MediaDeframer::new();
        let video = deframer.push(&stream);

        assert_eq!(video, expected);
        assert_eq!(deframer.dropped_bytes(), 0);
        assert_eq!(deframer.hints(), Some(MediaHints { fps: 12 }));
    }

    /// Captured audio-interleaved framing: audio is dropped, every video byte
    /// survives, and nothing desynchronizes.
    #[test]
    fn deframer_handles_captured_audio_interleaved_framing() {
        let (stream, expected) = stream_from_captured(captured_headers::WITH_AUDIO);

        let mut deframer = MediaDeframer::new();
        let video = deframer.push(&stream);

        assert_eq!(video, expected);
        assert_eq!(deframer.dropped_bytes(), 0);
        assert_eq!(deframer.hints(), Some(MediaHints { fps: 20 }));
    }

    /// The device splits its stream into fixed-size transport payloads that cut
    /// packets at arbitrary offsets. Replaying a captured stream in such slices
    /// must produce byte-identical output to one contiguous push.
    #[test]
    fn deframer_output_is_independent_of_chunk_boundaries() {
        let (stream, expected) = stream_from_captured(captured_headers::WITH_AUDIO);

        let mut deframer = MediaDeframer::new();
        let mut video = Vec::new();
        for chunk in stream.chunks(1024) {
            video.extend(deframer.push(chunk));
        }

        assert_eq!(video, expected);
        assert_eq!(deframer.dropped_bytes(), 0);
    }

    #[test]
    fn sofia_hash_matches_reference_vectors() {
        // "" -> "tlJwpbo6" is the widely published XMeye reference vector;
        // the rest were computed from the verified reference implementation.
        assert_eq!(sofia_hash(""), "tlJwpbo6");
        assert_eq!(sofia_hash("admin"), "6QNMIQGe");
        assert_eq!(sofia_hash("123456"), "nTBCS19C");
        assert_eq!(sofia_hash("12131412"), "lGUmOUsO");
    }

    #[test]
    fn sofia_hash_is_eight_ascii_alphanumeric_chars() {
        for password in ["", "a", "correct horse battery staple", "пароль"] {
            let hash = sofia_hash(password);
            assert_eq!(hash.len(), 8, "password {password:?} produced {hash:?}");
            assert!(hash.bytes().all(|b| b.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn encode_writes_header_length_and_terminator() {
        let frame = encode(command::LOGIN, 0, 0, "{}");
        assert_eq!(frame.len(), HEADER_LEN + 4); // "{}" + "\n\0"
        assert_eq!(frame[0], HEAD_FLAG);
        assert_eq!(u16::from_le_bytes([frame[14], frame[15]]), command::LOGIN);
        assert_eq!(
            u32::from_le_bytes([frame[16], frame[17], frame[18], frame[19]]),
            4
        );
        assert_eq!(&frame[HEADER_LEN..], b"{}\n\0");
    }

    #[test]
    fn encode_places_session_and_sequence_little_endian() {
        let frame = encode(command::USERS, 0x1234_5678, 0x00AB_CDEF, "{}");
        assert_eq!(
            u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
            0x1234_5678
        );
        assert_eq!(
            u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]),
            0x00AB_CDEF
        );
    }

    #[test]
    fn decode_round_trips_an_encoded_frame() {
        let json = r#"{"Name":"StorageInfo"}"#;
        let encoded = encode(command::CONFIG_QUERY, 0x0000_0036, 3, json);
        let frame = decode(&encoded).expect("decode");
        assert_eq!(
            frame,
            Frame {
                session: 0x0000_0036,
                sequence: 3,
                message_id: command::CONFIG_QUERY,
                payload: json.as_bytes().to_vec(),
            }
        );
    }

    #[test]
    fn decode_strips_only_trailing_terminators() {
        // A device reply whose JSON itself ends in `}` then `\0`.
        let mut buf = encode(command::USERS, 1, 1, r#"{"Ret":100}"#);
        // encode already appended \n\0; ensure they are stripped but JSON is intact.
        let frame = decode(&buf).expect("decode");
        assert_eq!(frame.payload, br#"{"Ret":100}"#);
        // Extra trailing NULs are also stripped.
        buf.push(0x00);
        // Fix the declared length to include the extra byte.
        let new_len = (u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) + 1).to_le_bytes();
        buf[16..20].copy_from_slice(&new_len);
        assert_eq!(decode(&buf).expect("decode").payload, br#"{"Ret":100}"#);
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert_eq!(decode(&[0xFF, 0x00]), Err(WireError::ShortHeader));
    }

    #[test]
    fn decode_rejects_bad_head_flag() {
        let mut buf = encode(command::LOGIN, 0, 0, "{}");
        buf[0] = 0x00;
        assert_eq!(decode(&buf), Err(WireError::BadHeadFlag(0x00)));
    }

    #[test]
    fn decode_rejects_truncated_payload() {
        let mut buf = encode(command::USERS, 0, 0, "{}");
        buf.truncate(HEADER_LEN + 1); // header declares 4 payload bytes, 1 present
        assert_eq!(
            decode(&buf),
            Err(WireError::TruncatedPayload {
                declared: 4,
                available: 1
            })
        );
    }
}
