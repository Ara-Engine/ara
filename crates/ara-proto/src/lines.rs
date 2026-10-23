//! Newline-delimited JSON framing (stdio transports, the MCP stdio adapter): one
//! message per line, UTF-8, no embedded newlines.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Reads the next non-empty line. `Ok(None)` at end of input.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_owned()));
        }
    }
}

/// Writes one message as a compact line and flushes.
pub fn write_message(writer: &mut impl Write, msg: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string(msg).map_err(io::Error::other)?;
    text.push('\n');
    writer.write_all(text.as_bytes())?;
    writer.flush()
}
