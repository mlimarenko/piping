use crate::errors::AppError;

// Guacamole status codes (must align with guacamole-common-js).
pub(crate) const GUAC_STATUS_UPSTREAM_TIMEOUT: u16 = 0x0202;
pub(crate) const GUAC_STATUS_UPSTREAM_ERROR: u16 = 0x0203;
pub(crate) const GUAC_STATUS_RESOURCE_CLOSED: u16 = 0x0206;
pub(crate) const GUAC_STATUS_UPSTREAM_UNAVAILABLE: u16 = 0x0208;
pub(crate) const GUAC_STATUS_CLIENT_BAD_TYPE: u16 = 0x030F;

/// Parses the `<digits>.` length prefix of one Guacamole element.
///
/// The declared length is a count of Unicode scalar values, not bytes, so the
/// returned byte offset is only the start of the value. A zero length is
/// rejected here because opcode helpers never inspect the empty internal-ping
/// opcode.
fn parse_length_prefix(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'.' {
        return None;
    }
    let len = text[..i].parse::<usize>().ok()?;
    if len == 0 {
        return None;
    }
    Some((len, i + 1))
}

/// Returns the byte offset immediately after `count` Unicode scalar values
/// starting at `start`, or `None` when the text holds fewer values.
fn byte_offset_after_chars(text: &str, start: usize, count: usize) -> Option<usize> {
    if count == 0 {
        return Some(start);
    }
    let mut chars = 0usize;
    for (offset, value) in text[start..].char_indices() {
        chars += 1;
        if chars == count {
            return Some(start + offset + value.len_utf8());
        }
    }
    None
}

pub(crate) fn looks_like_guacamole_instruction(text: &str) -> bool {
    // Guacamole protocol uses length-prefixed elements: "<len>.<data>,<len>.<data>;..."
    // Lengths count Unicode scalar values. Validate only the first element to
    // avoid ever touching sensitive args (e.g. passwords).
    let Some((len, start)) = parse_length_prefix(text) else {
        return false;
    };
    let Some(end) = byte_offset_after_chars(text, start, len) else {
        return false;
    };
    matches!(text.as_bytes().get(end), Some(b',' | b';'))
}

pub(crate) fn guac_opcode(text: &str) -> Option<&str> {
    // Only the opcode is inspected. Callers should first validate with
    // `looks_like_guacamole_instruction()` to avoid false positives.
    let (len, start) = parse_length_prefix(text)?;
    let end = byte_offset_after_chars(text, start, len)?;
    text.get(start..end)
}

pub(crate) fn is_guac_internal_ping(text: &str) -> bool {
    // WebSocketTunnel sends internal pings using empty opcode:
    // "0.,4.ping,<len>.<timestamp>;"
    text.starts_with("0.,4.ping,")
}

pub(crate) struct SliceBuffer {
    data: Vec<u8>,
    offset: usize,
}

impl SliceBuffer {
    pub(crate) fn new() -> Self {
        Self {
            data: Vec::new(),
            offset: 0,
        }
    }

    pub(crate) fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    pub(crate) fn available(&self) -> &[u8] {
        &self.data[self.offset..]
    }

    pub(crate) fn take_prefix(&self, len: usize) -> &[u8] {
        &self.data[self.offset..self.offset + len]
    }

    pub(crate) fn consume(&mut self, len: usize) {
        self.offset = self.offset.saturating_add(len);
        if self.offset == 0 {
            return;
        }
        // Avoid unbounded growth when consuming small chunks repeatedly.
        if self.offset >= 8192 && self.offset * 2 >= self.data.len() {
            self.data.drain(..self.offset);
            self.offset = 0;
        }
        if self.offset >= self.data.len() {
            self.data.clear();
            self.offset = 0;
        }
    }
}

pub(crate) fn encode_guac_instruction(elements: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for (idx, element) in elements.iter().enumerate() {
        // Guacamole lengths count Unicode scalar values, not UTF-8 bytes.
        out.extend_from_slice(element.chars().count().to_string().as_bytes());
        out.push(b'.');
        out.extend_from_slice(element.as_bytes());
        out.push(if idx + 1 == elements.len() {
            b';'
        } else {
            b','
        });
    }
    out
}

/// One decoded Guacamole instruction: byte length consumed and its elements.
type GuacInstruction = (usize, Vec<String>);

/// UTF-8 lead-byte width. Continuation bytes and overlong/invalid lead bytes
/// return `None`, so invalid framing is rejected instead of being skipped.
fn utf8_scalar_width(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

/// Parse one `<len>.<data><delimiter>` element starting at `offset`.
///
/// `<len>` is a count of Unicode scalar values, while the returned offset is a
/// byte offset. Returns `Ok(None)` when the buffer does not yet hold a complete
/// element (the caller waits for more bytes), `Ok(Some((next_offset, element,
/// delimiter)))` on success, and `Err` for malformed encoding, invalid UTF-8 or
/// an element whose declared length or encoded bytes exceed the bound.
fn parse_guac_element(buf: &[u8], offset: usize) -> Result<Option<(usize, String, u8)>, AppError> {
    const MAX_ELEMENT_BYTES: usize = crate::config::PROXY_WS_CONTROL_MAX_ELEMENT_BYTES;

    if offset >= buf.len() {
        return Ok(None);
    }

    let start_digits = offset;
    let mut cursor = offset;
    let max_length_digits = MAX_ELEMENT_BYTES.ilog10() as usize + 1;
    while cursor < buf.len() && buf[cursor].is_ascii_digit() {
        cursor += 1;
        if cursor - start_digits > max_length_digits {
            return Err(AppError::Validation(
                "Invalid guacamole element length".to_string(),
            ));
        }
    }
    if cursor == start_digits {
        return Err(AppError::Validation(
            "Invalid guacamole element encoding".to_string(),
        ));
    }
    if cursor >= buf.len() {
        return Ok(None);
    }
    if buf[cursor] != b'.' {
        return Err(AppError::Validation(
            "Invalid guacamole element encoding".to_string(),
        ));
    }
    let len = std::str::from_utf8(&buf[start_digits..cursor])
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| AppError::Validation("Invalid guacamole element length".to_string()))?;
    if len > MAX_ELEMENT_BYTES {
        return Err(AppError::Validation(
            "Guacamole element exceeds maximum size".to_string(),
        ));
    }

    let data_start = cursor + 1;
    let mut data_end = data_start;
    let mut chars = 0usize;
    while chars < len {
        if data_end >= buf.len() {
            return Ok(None);
        }
        let width = utf8_scalar_width(buf[data_end])
            .ok_or_else(|| AppError::Validation("Invalid guacamole element UTF-8".to_string()))?;
        if data_end + width > buf.len() {
            // The scalar is split across reads; wait for the remaining bytes.
            return Ok(None);
        }
        data_end += width;
        chars += 1;
        if data_end - data_start > MAX_ELEMENT_BYTES {
            return Err(AppError::Validation(
                "Guacamole element exceeds maximum size".to_string(),
            ));
        }
    }
    if data_end >= buf.len() {
        return Ok(None);
    }
    let delimiter = buf[data_end];
    let data = std::str::from_utf8(&buf[data_start..data_end])
        .map_err(|_| AppError::Validation("Invalid guacamole element UTF-8".to_string()))?;
    Ok(Some((data_end + 1, data.to_string(), delimiter)))
}

pub(crate) fn parse_guac_instruction(buf: &[u8]) -> Result<Option<GuacInstruction>, AppError> {
    const MAX_ELEMENTS: usize = crate::config::PROXY_WS_CONTROL_MAX_ELEMENTS;

    let mut offset = 0usize;
    let mut elements: Vec<String> = Vec::new();
    loop {
        if elements.len() >= MAX_ELEMENTS {
            return Err(AppError::Validation(
                "Guacamole instruction has too many elements".to_string(),
            ));
        }
        let (next_offset, element, delimiter) = match parse_guac_element(buf, offset)? {
            Some(parsed) => parsed,
            None => return Ok(None),
        };
        offset = next_offset;
        elements.push(element);

        match delimiter {
            b',' => continue,
            b';' => return Ok(Some((offset, elements))),
            _ => {
                return Err(AppError::Validation(
                    "Invalid guacamole element delimiter".to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_single_instruction() {
        let encoded = encode_guac_instruction(&["4".to_string(), "size".to_string()]);
        let (consumed, elements) = parse_guac_instruction(&encoded)
            .expect("valid instruction")
            .expect("complete instruction");
        assert_eq!(consumed, encoded.len());
        assert_eq!(elements, vec!["4".to_string(), "size".to_string()]);
    }

    #[test]
    fn returns_none_for_a_partial_element() {
        // Declares a 4-byte element but only two bytes are present.
        assert!(parse_guac_instruction(b"4.si").expect("partial").is_none());
    }

    #[test]
    fn returns_none_for_an_incomplete_length_prefix() {
        assert!(parse_guac_instruction(b"").expect("empty").is_none());
        assert!(
            parse_guac_instruction(b"12")
                .expect("digits only")
                .is_none()
        );
    }

    #[test]
    fn errors_on_a_missing_dot_after_the_length() {
        let error = parse_guac_instruction(b"4xabcd;").expect_err("must reject");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn rejects_unbounded_prefixes_and_invalid_utf8() {
        let digits = vec![
            b'9';
            crate::config::PROXY_WS_CONTROL_MAX_ELEMENT_BYTES
                .to_string()
                .len()
                + 1
        ];
        assert!(parse_guac_instruction(&digits).is_err());
        assert!(parse_guac_instruction(b"x").is_err());
        assert!(parse_guac_instruction(b"1.\xff;").is_err());
        let overflow = format!("{}.a;", usize::MAX);
        assert!(!looks_like_guacamole_instruction(&overflow));
        assert!(guac_opcode(&overflow).is_none());
    }

    #[test]
    fn errors_on_an_oversized_element_length() {
        let oversized = format!("{}.", crate::config::PROXY_WS_CONTROL_MAX_ELEMENT_BYTES + 1);
        let error = parse_guac_instruction(oversized.as_bytes()).expect_err("must reject");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn errors_on_an_invalid_delimiter() {
        let error = parse_guac_instruction(b"1.aX").expect_err("must reject");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn errors_when_too_many_elements_are_present() {
        let elements = vec!["a".to_string(); crate::config::PROXY_WS_CONTROL_MAX_ELEMENTS + 1];
        let encoded = encode_guac_instruction(&elements);
        let error = parse_guac_instruction(&encoded).expect_err("must reject");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn element_lengths_count_unicode_scalars_not_bytes() {
        let encoded = encode_guac_instruction(&["é".to_string(), "🖥".to_string()]);
        assert_eq!(encoded, "1.é,1.🖥;".as_bytes());

        let (consumed, elements) = parse_guac_instruction(&encoded)
            .expect("valid instruction")
            .expect("complete instruction");
        assert_eq!(consumed, encoded.len());
        assert_eq!(elements, vec!["é".to_string(), "🖥".to_string()]);
    }

    #[test]
    fn opcode_helpers_use_unicode_scalar_lengths() {
        assert!(looks_like_guacamole_instruction("1.é;"));
        assert_eq!(guac_opcode("1.é;"), Some("é"));
        assert!(looks_like_guacamole_instruction("2.éa,1.b;"));
        assert_eq!(guac_opcode("2.éa,1.b;"), Some("éa"));
        // The same bytes with a byte-length prefix would frame incorrectly.
        assert!(!looks_like_guacamole_instruction("2.é;"));
    }

    #[test]
    fn waits_for_a_split_multibyte_scalar_without_erroring() {
        let encoded = encode_guac_instruction(&["aé".to_string()]);
        let scalar_start = encoded
            .iter()
            .position(|byte| *byte == 0xC3)
            .expect("encoded scalar");
        for split in [scalar_start, scalar_start + 1] {
            assert!(
                parse_guac_instruction(&encoded[..split])
                    .expect("partial prefix is not an error")
                    .is_none(),
                "split at {split} must wait for more bytes"
            );
        }
        let (consumed, elements) = parse_guac_instruction(&encoded)
            .expect("valid instruction")
            .expect("complete instruction");
        assert_eq!(consumed, encoded.len());
        assert_eq!(elements, vec!["aé".to_string()]);
    }

    #[test]
    fn rejects_invalid_utf8_scalar_leads_and_continuations() {
        assert!(parse_guac_instruction(b"1.\xff;").is_err());
        assert!(parse_guac_instruction(b"1.\xc3x;").is_err());
        assert!(parse_guac_instruction(b"1.\x80;").is_err());
    }
}
