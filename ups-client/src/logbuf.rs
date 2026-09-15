//! In-memory ring buffer of recent log lines, feeding the in-app "Console"
//! window opened from the tray menu.
//!
//! A real OS console was tried first, but Windows always terminates
//! whatever's attached to a console when its window is closed — there's no
//! reliable way to intercept that (a `CTRL_CLOSE_EVENT` handler only buys a
//! moment for cleanup, not survival), and on machines where Windows Terminal
//! is the default terminal app, the actual window the user clicks isn't even
//! the one `GetConsoleWindow` returns, so disabling its close button doesn't
//! help either. An in-app window sidesteps all of that.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const MAX_LINES: usize = 2000;

fn buffer() -> &'static Mutex<VecDeque<String>> {
    static BUF: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LINES)))
}

fn push(line: &str) {
    if let Ok(mut buf) = buffer().lock() {
        buf.push_back(line.to_string());
        while buf.len() > MAX_LINES {
            buf.pop_front();
        }
    }
}

/// All buffered lines, oldest first.
pub fn snapshot() -> Vec<String> {
    buffer()
        .lock()
        .map(|b| b.iter().cloned().collect())
        .unwrap_or_default()
}

/// A `tracing_subscriber` writer that appends formatted lines to the buffer.
/// Combine with other writers via `MakeWriterExt::and`.
#[derive(Clone, Default)]
pub struct BufferWriter;

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for line in String::from_utf8_lossy(buf).lines() {
            if !line.is_empty() {
                push(line);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BufferWriter
    }
}
