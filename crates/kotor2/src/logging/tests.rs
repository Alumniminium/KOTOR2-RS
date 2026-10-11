use std::io::Write;
use std::sync::Arc;

use super::*;

/// A fmt writer that keeps everything in memory.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_console_filter_changes_while_the_subscriber_runs() {
    let out = Buffer::default();
    let writer = out.clone();
    let (subscriber, control) = compose(move || writer.clone(), "info", false);
    let _guard = subscriber.set_default();
    bevy::log::info!("visible at info");
    bevy::log::debug!("hidden at info");
    (control.set_filter)(EnvFilter::new("debug")).unwrap();
    bevy::log::debug!("visible at debug");
    (control.set_filter)(EnvFilter::new("error")).unwrap();
    bevy::log::info!("hidden at error");
    let text = out.text();
    assert!(text.contains("visible at info"), "{text}");
    assert!(!text.contains("hidden at info"), "{text}");
    assert!(text.contains("visible at debug"), "{text}");
    assert!(!text.contains("hidden at error"), "{text}");
}

#[test]
fn the_trace_layer_has_its_own_filter() {
    let console = Buffer::default();
    let trace = Buffer::default();
    let console_writer = console.clone();
    let (subscriber, control) = compose(move || console_writer.clone(), "error", false);
    let _guard = subscriber.set_default();
    (control.set_trace_filter)(EnvFilter::new("info")).unwrap();
    control.trace.set(Some(Box::new(trace.clone())));
    bevy::log::info!("info reaches only the trace");
    bevy::log::error!("errors reach both");
    (control.set_trace_filter)(EnvFilter::new("off")).unwrap();
    control.trace.set(None);
    bevy::log::info!("after the trace");
    assert!(
        trace.text().contains("info reaches only the trace"),
        "{}",
        trace.text()
    );
    assert!(
        trace.text().contains("errors reach both"),
        "{}",
        trace.text()
    );
    assert!(
        !trace.text().contains("after the trace"),
        "{}",
        trace.text()
    );
    assert!(
        !console.text().contains("info reaches only the trace"),
        "{}",
        console.text()
    );
    assert!(
        console.text().contains("errors reach both"),
        "{}",
        console.text()
    );
}

#[test]
fn colour_needs_a_terminal_and_no_no_color() {
    assert!(wants_ansi(true, false));
    assert!(
        !wants_ansi(false, false),
        "a pipe or a file gets plain text"
    );
    assert!(!wants_ansi(true, true), "NO_COLOR turns colour off");
}

#[test]
fn a_trace_never_truncates_an_existing_file() {
    let dir = std::env::temp_dir().join(format!("kotor2-trace-keep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("save.json");
    std::fs::write(&path, b"precious").unwrap();
    // Refused before any control handle is needed: the file is opened first.
    assert!(start_trace(&path, TRACE_FILTER).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"precious");
    let _ = std::fs::remove_dir_all(&dir);
}
