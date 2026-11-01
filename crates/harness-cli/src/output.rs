use std::{
    io::{self, Write},
    sync::{Arc, Condvar, Mutex},
};
use tokio::sync::{oneshot, watch};
/// The output thread may block in the OS. It never owns the engine, cancellation,
/// SQLite or cleanup. Only the newest progress snapshot waits behind a blocked
/// write. Initial and terminal records are retained separately from progress.
pub struct Output {
    pending: Arc<(Mutex<Pending>, Condvar)>,
    pub failed: watch::Receiver<bool>,
}
#[derive(Default)]
struct Pending {
    initial: Option<String>,
    latest: Option<(String, Option<oneshot::Sender<()>>)>,
    closed: bool,
}
impl Output {
    pub fn new(json: bool) -> Self {
        Self::with_writer(io::stdout(), json)
    }
    fn with_writer(mut stdout: impl Write + Send + 'static, json: bool) -> Self {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker = pending.clone();
        let (failure, failed) = watch::channel(false);
        std::thread::spawn(move || {
            let mut rendered = String::new();
            loop {
                let (text, ack, initial) = {
                    let (mutex, ready) = &*worker;
                    let mut state = mutex.lock().unwrap();
                    while state.initial.is_none() && state.latest.is_none() && !state.closed {
                        state = ready.wait(state).unwrap();
                    }
                    if let Some(text) = state.initial.take() {
                        (text, None, true)
                    } else if let Some((text, ack)) = state.latest.take() {
                        (text, ack, false)
                    } else {
                        break;
                    }
                };
                // Human snapshots contain the full bounded transcript; JSON
                // progress records are independent versioned snapshots.
                let bytes = if json || initial {
                    text.as_str()
                } else {
                    text.strip_prefix(&rendered).unwrap_or(&text)
                };
                if stdout
                    .write_all(bytes.as_bytes())
                    .and_then(|_| stdout.flush())
                    .is_err()
                {
                    failure.send_replace(true);
                    break;
                }
                if !json && !initial {
                    rendered = text;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                    break;
                }
            }
        });
        Self { pending, failed }
    }
    pub fn start(&self, text: String) -> Result<(), String> {
        let (mutex, ready) = &*self.pending;
        let mut state = mutex.lock().unwrap();
        if *self.failed.borrow() || state.closed {
            return Err("output observer unavailable".into());
        }
        state.initial = Some(text);
        ready.notify_one();
        Ok(())
    }
    pub fn send(&self, text: String) -> Result<(), String> {
        let (mutex, ready) = &*self.pending;
        let mut state = mutex.lock().unwrap();
        if *self.failed.borrow() || state.closed {
            return Err("output observer unavailable".into());
        }
        state.latest = Some((text, None));
        ready.notify_one();
        Ok(())
    }
    pub async fn finish(&self, text: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        {
            let (mutex, ready) = &*self.pending;
            let mut state = mutex.lock().unwrap();
            if *self.failed.borrow() || state.closed {
                return Err("output observer unavailable".into());
            }
            state.latest = Some((text, Some(tx)));
            state.closed = true;
            ready.notify_one();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .map_err(|_| "output blocked; engine cleanup already complete")?
            .map_err(|_| "output failed".into())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let (mutex, ready) = &*self.pending;
        mutex.lock().unwrap().closed = true;
        ready.notify_one();
    }
}

/// Bounded by the turn's output/request limits. Snapshots may skip deltas or
/// whole attempts; identity comes from the engine, never from text equality.
#[derive(Default)]
pub struct Transcript {
    attempt: usize,
    discarded: usize,
    current: String,
    pub text: String,
}
impl Transcript {
    pub fn observe(&mut self, progress: &harness_core::Progress) {
        if progress.attempt != self.attempt {
            if !self.current.is_empty() {
                self.text.push('\n');
            }
            self.current.clear();
            self.attempt = progress.attempt;
            if self.attempt > 0 {
                self.text
                    .push_str(&format!("[request {} started]\n", self.attempt));
            }
        }
        if progress.discarded_attempts > self.discarded {
            self.text
                .push_str("\n[request attempt discarded; prior partial output is incomplete]\n");
            self.discarded = progress.discarded_attempts;
        }
        self.append(&progress.text);
    }
    pub fn append(&mut self, text: &str) {
        if let Some(suffix) = text.strip_prefix(&self.current) {
            self.text.push_str(&escaped(suffix));
        } else {
            self.text.push_str("\n[updated response]\n");
            self.text.push_str(&escaped(text));
        }
        self.current = text.to_owned();
    }
}

pub fn escaped(text: &str) -> String {
    text.chars().flat_map(|c|{
    if c.is_control() && !matches!(c,'\n'|'\t') || matches!(c,'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}') {c.escape_unicode().collect::<Vec<_>>()}else{vec![c]}
}).collect()
}

/// JSON control escapes also cover bidi/invisible characters that serde_json
/// otherwise leaves literal. The output remains valid NDJSON.
pub fn json_line(value: serde_json::Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if matches!(c,'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}')
        {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    struct BlockedWriter {
        entered: Option<oneshot::Sender<()>>,
        release: mpsc::Receiver<()>,
        bytes: Arc<Mutex<Vec<u8>>>,
    }
    impl Write for BlockedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
                self.release.recv().unwrap();
            }
            self.bytes.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn stalled_output_coalesces_without_losing_terminal() {
        let (entered, ready) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let output = Output::with_writer(
            BlockedWriter {
                entered: Some(entered),
                release: wait,
                bytes: bytes.clone(),
            },
            false,
        );
        output.start("initial\n".into()).unwrap();
        ready.await.unwrap();
        let mut transcript = Transcript::default();
        transcript.observe(&harness_core::Progress {
            attempt: 1,
            ..Default::default()
        });
        assert_eq!(transcript.text, "[request 1 started]\n");
        transcript.observe(&harness_core::Progress {
            attempt: 1,
            text: "Inspecting.".into(),
            ..Default::default()
        });
        output.send(transcript.text.clone()).unwrap();
        let results: Vec<_> = (0..64)
            .map(|n| {
                transcript.observe(&harness_core::Progress {
                    attempt: 2,
                    text: "a".repeat(n),
                    ..Default::default()
                });
                output.send(transcript.text.clone())
            })
            .collect();
        assert!(
            !transcript.text.contains("discarded"),
            "a normal tool continuation is not a retry"
        );
        transcript.observe(&harness_core::Progress {
            attempt: 3,
            discarded_attempts: 1,
            text: "Answer".into(),
            ..Default::default()
        });
        output.send(transcript.text.clone()).unwrap();
        transcript.append("Answer complete.");
        release.send(()).unwrap();
        assert!(
            results.iter().all(Result::is_ok),
            "backpressure must coalesce, not disconnect"
        );
        output
            .finish(format!("{}\nterminal\n", transcript.text))
            .await
            .unwrap();
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert_eq!(
            text,
            format!(
                "initial\n[request 1 started]\nInspecting.\n[request 2 started]\n{}\n[request 3 started]\n\n[request attempt discarded; prior partial output is incomplete]\nAnswer complete.\nterminal\n",
                "a".repeat(63)
            )
        );
    }
}
