//! Logging for the game. A tracing subscriber with two layers, each with its
//! own filter: the console (stderr) and a JSON trace file. set_log_filter
//! changes the console filter while the game runs. trace_start and trace_stop
//! add and remove the trace layer, whose filter is given with trace_start.
//! They reach the subscriber through the control socket (remote.rs). It
//! replaces Bevy's LogPlugin, which fixes its filter at startup (main.rs
//! disables it).
//!
//! The console matches Bevy's default: INFO plus bevy::log::DEFAULT_FILTER,
//! RUST_LOG added on top. Colour only on a terminal, and never with NO_COLOR
//! set. The trace file is JSON, one object per event, with span close events
//! carrying busy and idle times.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, reload};

/// Filter of a trace started without one.
pub const TRACE_FILTER: &str = "info,kotor2=trace,k2_world=trace";

/// Where the trace writes: the trace file, or nothing.
type TraceOut = Option<Box<dyn Write + Send>>;

/// The trace layer's writer. It writes to what the control set, and discards
/// events while nothing is set. The layer itself stays in the subscriber for
/// the whole run, so its filter can change (a layer added later would not get
/// its per-layer filter).
#[derive(Clone, Default)]
struct TraceSink(Arc<Mutex<TraceOut>>);

impl TraceSink {
    fn set(&self, out: TraceOut) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = out;
    }
}

impl Write for TraceSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Some(out) => out.write(buf),
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Some(out) => out.flush(),
            None => Ok(()),
        }
    }
}

/// Handles that change the running subscriber.
struct Control {
    set_filter: Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>,
    set_trace_filter: Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>,
    trace: TraceSink,
}

static CONTROL: OnceLock<Control> = OnceLock::new();
static FILTER_TEXT: Mutex<String> = Mutex::new(String::new());
static TRACE_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The filter Bevy would use: INFO and its default per-crate levels, then RUST_LOG.
pub fn default_filter_text() -> String {
    let mut text = format!("{},{}", bevy::log::Level::INFO, bevy::log::DEFAULT_FILTER);
    if let Ok(env) = std::env::var(EnvFilter::DEFAULT_ENV) {
        if !env.trim().is_empty() {
            text.push(',');
            text.push_str(env.trim());
        }
    }
    text
}

/// Colour on the console: only on a terminal, and only when NO_COLOR is unset.
fn wants_ansi(terminal: bool, no_color: bool) -> bool {
    terminal && !no_color
}

/// Builds the subscriber, with `writer` for the console layer, and the handles
/// that change it. Used by install() and by the tests. Both layers have their
/// own filter: the console's is `filter_text`, the trace's starts off.
fn compose<W>(writer: W, filter_text: &str, ansi: bool) -> (impl SubscriberInitExt, Control)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let (console_filter, console_handle) = reload::Layer::new(EnvFilter::builder().parse_lossy(filter_text));
    let console = fmt::layer().with_ansi(ansi).with_writer(writer).with_filter(console_filter);
    let trace = TraceSink::default();
    let (trace_filter, trace_handle) = reload::Layer::new(EnvFilter::new("off"));
    let trace_layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_span_events(FmtSpan::CLOSE)
        .with_writer({
            let sink = trace.clone();
            move || sink.clone()
        })
        .with_filter(trace_filter);
    let subscriber = Registry::default().with(console).with(trace_layer);
    let control = Control {
        set_filter: Box::new(move |f| console_handle.reload(f).map_err(|e| e.to_string())),
        set_trace_filter: Box::new(move |f| trace_handle.reload(f).map_err(|e| e.to_string())),
        trace,
    };
    (subscriber, control)
}

/// Installs the global subscriber. Call once, before the App is built.
pub fn install() {
    let text = default_filter_text();
    let ansi = wants_ansi(std::io::stderr().is_terminal(), std::env::var_os("NO_COLOR").is_some());
    let (subscriber, control) = compose(std::io::stderr, &text, ansi);
    if subscriber.try_init().is_err() {
        eprintln!("logging: a tracing subscriber is already installed");
    }
    *FILTER_TEXT.lock().unwrap_or_else(|e| e.into_inner()) = text;
    let _ = CONTROL.set(control);
}

fn control() -> Result<&'static Control, String> {
    CONTROL.get().ok_or_else(|| "logging is not installed".to_owned())
}

/// Replaces the console filter. Bad directives are refused and the old filter stays.
pub fn set_filter(text: &str) -> Result<(), String> {
    let filter = EnvFilter::try_new(text).map_err(|e| format!("bad filter {text:?}: {e}"))?;
    (control()?.set_filter)(filter)?;
    *FILTER_TEXT.lock().unwrap_or_else(|e| e.into_inner()) = text.to_owned();
    Ok(())
}

/// The console filter in force.
pub fn filter_text() -> String {
    FILTER_TEXT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts writing a JSON trace to `path`, filtered by `filter_text`, replacing
/// any trace in progress. The file is created new: an existing file is refused,
/// never truncated.
pub fn start_trace(path: &Path, filter_text: &str) -> Result<(), String> {
    let filter = EnvFilter::try_new(filter_text).map_err(|e| format!("bad trace filter {filter_text:?}: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let c = control()?;
    (c.set_trace_filter)(filter)?;
    c.trace.set(Some(Box::new(file)));
    *TRACE_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_owned());
    Ok(())
}

/// Stops the trace in progress; returns its file, if one was running.
pub fn stop_trace() -> Result<Option<PathBuf>, String> {
    let path = TRACE_FILE.lock().unwrap_or_else(|e| e.into_inner()).take();
    if path.is_some() {
        let c = control()?;
        (c.set_trace_filter)(EnvFilter::new("off"))?;
        c.trace.set(None);
    }
    Ok(path)
}

/// The trace file being written, if any.
pub fn trace_file() -> Option<PathBuf> {
    TRACE_FILE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests;
