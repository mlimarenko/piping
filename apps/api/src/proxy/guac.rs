use crate::errors::AppError;

// Guacamole status codes (must align with guacamole-common-js).
pub(crate) const GUAC_STATUS_SERVER_ERROR: u16 = 0x0200;
pub(crate) const GUAC_STATUS_UPSTREAM_TIMEOUT: u16 = 0x0202;
pub(crate) const GUAC_STATUS_UPSTREAM_ERROR: u16 = 0x0203;
pub(crate) const GUAC_STATUS_RESOURCE_CLOSED: u16 = 0x0206;
pub(crate) const GUAC_STATUS_UPSTREAM_UNAVAILABLE: u16 = 0x0208;
pub(crate) const GUAC_STATUS_CLIENT_BAD_REQUEST: u16 = 0x0300;
pub(crate) const GUAC_STATUS_CLIENT_BAD_TYPE: u16 = 0x030F;

pub(crate) fn looks_like_guacamole_instruction(text: &str) -> bool {
    // Guacamole protocol uses length-prefixed elements: "<len>.<data>,<len>.<data>;..."
    // Validate only the first element to avoid ever touching sensitive args (e.g. passwords).
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'.' {
        return false;
    }
    let Ok(len) = text[..i].parse::<usize>() else {
        return false;
    };
    if len == 0 {
        return false;
    }
    let start = i + 1;
    if bytes.len() <= start + len {
        return false;
    }
    matches!(bytes[start + len], b',' | b';')
}

pub(crate) fn guac_opcode(text: &str) -> Option<&str> {
    // Only the opcode is inspected. Callers should first validate with
    // `looks_like_guacamole_instruction()` to avoid false positives.
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
    let start = i + 1;
    if bytes.len() < start + len {
        return None;
    }
    text.get(start..start + len)
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
        let bytes = element.as_bytes();
        out.extend_from_slice(bytes.len().to_string().as_bytes());
        out.push(b'.');
        out.extend_from_slice(bytes);
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

/// Parse one `<len>.<data><delimiter>` element starting at `offset`.
///
/// Returns `Ok(None)` when the buffer does not yet hold a complete element (the
/// caller waits for more bytes), `Ok(Some((next_offset, element, delimiter)))`
/// on success, and `Err` for a malformed encoding or an oversized element.
fn parse_guac_element(buf: &[u8], offset: usize) -> Result<Option<(usize, String, u8)>, AppError> {
    const MAX_ELEMENT_BYTES: usize = crate::config::PROXY_WS_CONTROL_MAX_ELEMENT_BYTES;

    if offset >= buf.len() {
        return Ok(None);
    }

    let start_digits = offset;
    let mut cursor = offset;
    while cursor < buf.len() && buf[cursor].is_ascii_digit() {
        cursor += 1;
    }
    if cursor == start_digits || cursor >= buf.len() {
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

    let data_start = cursor + 1; // skip '.'
    if buf.len() < data_start + len + 1 {
        return Ok(None);
    }
    let data = &buf[data_start..data_start + len];
    let delimiter = buf[data_start + len];
    Ok(Some((
        data_start + len + 1,
        String::from_utf8_lossy(data).to_string(),
        delimiter,
    )))
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
}
