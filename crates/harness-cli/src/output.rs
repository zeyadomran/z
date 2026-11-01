use std::{
    io::{self, Write},
    sync::mpsc,
};
use tokio::sync::{oneshot, watch};
/// The output thread may block in the OS. It never owns the engine, cancellation,
/// SQLite or cleanup, and a full presentation queue is observer loss.
pub struct Output {
    sender: mpsc::SyncSender<(String, Option<oneshot::Sender<()>>)>,
    pub failed: watch::Receiver<bool>,
}
impl Output {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<(String, Option<oneshot::Sender<()>>)>(16);
        let (failure, failed) = watch::channel(false);
        std::thread::spawn(move || {
            let mut stdout = io::stdout().lock();
            while let Ok((text, ack)) = receiver.recv() {
                if stdout
                    .write_all(text.as_bytes())
                    .and_then(|_| stdout.flush())
                    .is_err()
                {
                    failure.send_replace(true);
                    break;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            }
        });
        Self { sender, failed }
    }
    pub fn send(&self, text: String) -> Result<(), String> {
        self.sender
            .try_send((text, None))
            .map_err(|_| "output observer unavailable".into())
    }
    pub async fn finish(&self, text: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .try_send((text, Some(tx)))
            .map_err(|_| "output observer unavailable")?;
        tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .map_err(|_| "output blocked; engine cleanup already complete")?
            .map_err(|_| "output failed".into())
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
