//! Incremental framing for the station's LF/CRLF-delimited HTTP event stream.

/// One dispatched event. The identifier persists until a later `id` field,
/// including an empty one; callers decide when to commit their replay cursor.
#[derive(Debug, PartialEq, Eq)]
pub struct ServerSentEvent {
    pub kind: String,
    pub identifier: Option<String>,
    pub data: String,
}

/// Buffers incomplete lines and dispatches only data-bearing events terminated
/// by a blank line. Dropping a decoder discards any incomplete final event.
#[derive(Default)]
pub struct ServerSentEventDecoder {
    buffer: Vec<u8>,
    consumed_bytes: usize,
    kind: String,
    identifier: Option<String>,
    data: Option<String>,
}

impl ServerSentEventDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.drain(..self.consumed_bytes);
        self.consumed_bytes = 0;
        self.buffer.extend_from_slice(bytes);
    }

    /// Read the next complete event, retaining incomplete input for `push`.
    ///
    /// # Errors
    /// A complete line contains invalid UTF-8. Discard the decoder on error.
    pub fn next_event(&mut self) -> Result<Option<ServerSentEvent>, std::str::Utf8Error> {
        while let Some(line_length) = self.buffer[self.consumed_bytes..]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let line_end = self.consumed_bytes + line_length;
            let line = std::str::from_utf8(&self.buffer[self.consumed_bytes..line_end])?;
            let line = line.strip_suffix('\r').unwrap_or(line);
            self.consumed_bytes = line_end + 1;
            if line.is_empty() {
                let kind = std::mem::take(&mut self.kind);
                if let Some(data) = self.data.take() {
                    return Ok(Some(ServerSentEvent {
                        kind,
                        identifier: self.identifier.clone(),
                        data,
                    }));
                }
            } else if !line.starts_with(':') {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => value.clone_into(&mut self.kind),
                    "id" if !value.contains('\0') => self.identifier = Some(value.to_owned()),
                    "data" => match &mut self.data {
                        Some(data) => {
                            data.push('\n');
                            data.push_str(value);
                        }
                        None => self.data = Some(value.to_owned()),
                    },
                    _ => {}
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerSentEvent, ServerSentEventDecoder};

    #[test]
    fn network_chunk_boundaries_do_not_change_delivered_events() {
        let stream = ": heartbeat\r\nid: station:1\r\nevent: snapshot\r\ndata: café\r\ndata:  second line\r\n\r\ndata: next\n\n".as_bytes();
        let expected = vec![
            ServerSentEvent {
                kind: "snapshot".to_owned(),
                identifier: Some("station:1".to_owned()),
                data: "café\n second line".to_owned(),
            },
            ServerSentEvent {
                kind: String::new(),
                identifier: Some("station:1".to_owned()),
                data: "next".to_owned(),
            },
        ];
        for split_at in 0..=stream.len() {
            let mut decoder = ServerSentEventDecoder::default();
            let mut delivered = Vec::new();
            for chunk in [&stream[..split_at], &stream[split_at..]] {
                decoder.push(chunk);
                while let Some(event) = decoder.next_event().unwrap() {
                    delivered.push(event);
                }
            }
            assert_eq!(delivered, expected, "split at byte {split_at}");
        }
    }

    #[test]
    fn identifier_only_events_and_invalid_identifiers_preserve_cursor_rules() {
        let mut decoder = ServerSentEventDecoder::default();
        decoder.push(b"id: first\nevent: discarded\n\nid: invalid\0value\ndata:\n\nid\ndata: after reset\n\ndata: incomplete");
        assert_eq!(
            decoder.next_event().unwrap(),
            Some(ServerSentEvent {
                kind: String::new(),
                identifier: Some("first".to_owned()),
                data: String::new(),
            })
        );
        assert_eq!(
            decoder.next_event().unwrap(),
            Some(ServerSentEvent {
                kind: String::new(),
                identifier: Some(String::new()),
                data: "after reset".to_owned(),
            })
        );
        assert!(decoder.next_event().unwrap().is_none());
    }

    #[test]
    fn malformed_later_input_does_not_prevent_delivery_of_an_earlier_event() {
        let mut decoder = ServerSentEventDecoder::default();
        decoder.push(b"data: valid\n\ndata: \xff\n\n");
        assert_eq!(decoder.next_event().unwrap().unwrap().data, "valid");
        assert!(decoder.next_event().is_err());
    }
}
