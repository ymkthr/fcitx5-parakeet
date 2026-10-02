//! Unix-socket server speaking a one-line-per-message text protocol.
//!
//! Request:   `<id> <COMMAND> [args]`
//! Response:  `<id> OK [payload]` or `<id> ERR <message>`
//! Event:     `<id> PARTIAL <text>`, pushed for an earlier request `<id>`
//!
//! Commands:
//!   HELLO            -> `OK parakeetd <version>`
//!   LOAD <lang>      -> `OK` once the model(s) for <lang> are in memory
//!   START <lang>     -> `OK` once the microphone capture is running. Until the
//!                       capture ends, `<id> PARTIAL <text>` lines (same id)
//!                       carry a provisional transcript of the audio so far
//!                       whenever it changes; none follow the STOP/CANCEL reply
//!   STOP [context]   -> `OK <lang> <text>` after transcription (text may be empty);
//!                       context is the text before the cursor, used to correct
//!                       Japanese homophones
//!   CANCEL           -> `OK` (drops the current capture)
//!   STATUS           -> `OK recording=<0|1> loaded=<a,b> languages=<a,b>`
//!
//! `<lang>` is a configured model language (`ja`, `en`) or `auto`, which
//! decodes with both models and reports the language it settled on.
//!
//! Only one capture can run at a time (the microphone is shared); the
//! connection that issued START owns it, and closing that connection cancels it.

use std::os::fd::FromRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info, warn};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;

use crate::asr::{self, Gate, Pool};
use crate::audio::{Capture, CaptureConfig, Recording};
use crate::config::{Config, AUTO_LANG};
use crate::correct::Corrector;

/// Seconds to wait for the first samples before declaring the microphone dead.
const RECORDER_START_TIMEOUT: Duration = Duration::from_secs(3);
/// The preview re-decodes its tail every tick; past this it commits up to a pause.
const PREVIEW_TAIL_SECONDS: f32 = 10.0;
/// Audio a preview cut must leave after it, so a word just begun is not split.
const PREVIEW_MIN_AFTER_SECONDS: f32 = 1.0;

struct Active {
    owner: u64,
    lang: String,
    recording: Option<Arc<Recording>>,
    /// Dropping it stops the capture's preview task.
    preview: oneshot::Sender<()>,
}

pub struct Daemon {
    cfg: Config,
    pool: Arc<Pool>,
    gate: Option<Arc<Gate>>,
    corrector: Option<Arc<Corrector>>,
    capture: Mutex<Option<Capture>>,
    active: Mutex<Option<Active>>,
    next_conn: AtomicU64,
}

type Writer = Arc<tokio::sync::Mutex<OwnedWriteHalf>>;

async fn send(w: &Writer, line: String) {
    write_line(&mut *w.lock().await, line).await;
}

async fn write_line(w: &mut OwnedWriteHalf, line: String) {
    if let Err(e) = w.write_all(format!("{line}\n").as_bytes()).await {
        debug!("client write failed: {e}");
    }
}

async fn ok(w: &Writer, id: &str, payload: &str) {
    send(w, format!("{id} OK {payload}").trim_end().to_string()).await;
}

async fn err(w: &Writer, id: &str, message: &str) {
    send(w, format!("{id} ERR {}", asr::sanitize(message))).await;
}

/// Splits `<id> <COMMAND> [args]`; the command is case-insensitive.
fn parse_request(line: &str) -> (&str, String, &str) {
    let mut parts = line.splitn(3, ' ');
    let id = parts.next().unwrap_or("0");
    let command = parts.next().unwrap_or("").to_ascii_uppercase();
    (id, command, parts.next().unwrap_or("").trim())
}

impl Daemon {
    pub fn new(cfg: Config) -> Arc<Self> {
        let gate = cfg.vad_model.as_ref().and_then(|path| {
            match Gate::new(path, cfg.sample_rate, cfg.max_seconds) {
                Ok(g) => Some(Arc::new(g)),
                Err(e) => {
                    warn!("{e}; noise-only captures may produce filler words");
                    None
                }
            }
        });
        let pool = Arc::new(Pool::new(&cfg));
        let corrector = cfg.correction.clone().map(|c| Arc::new(Corrector::new(c)));
        let capture = match Capture::spawn(CaptureConfig {
            sample_rate: cfg.sample_rate,
            target: cfg.target.clone(),
        }) {
            Ok(c) => Some(c),
            Err(e) => {
                error!("{e}; will retry on the first capture request");
                None
            }
        };
        Arc::new(Self {
            cfg,
            pool,
            gate,
            corrector,
            capture: Mutex::new(capture),
            active: Mutex::new(None),
            next_conn: AtomicU64::new(1),
        })
    }

    pub async fn preload(self: &Arc<Self>) {
        for lang in self.cfg.preload.clone() {
            let pool = Arc::clone(&self.pool);
            let l = lang.clone();
            match tokio::task::spawn_blocking(move || pool.get(&l)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => error!("preload {lang} failed: {e:#}"),
                Err(e) => error!("preload {lang} panicked: {e}"),
            }
        }
        if let Some(corrector) = self.corrector.clone() {
            if self.cfg.preload.iter().any(|l| l == "ja") {
                match tokio::task::spawn_blocking(move || corrector.load()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => error!("preload correction failed: {e:#}"),
                    Err(e) => error!("preload correction panicked: {e}"),
                }
            }
        }
    }

    pub async fn serve(self: Arc<Self>, listener: UnixListener) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let daemon = Arc::clone(&self);
                    tokio::spawn(async move { daemon.handle(stream).await });
                }
                Err(e) => {
                    warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    async fn handle(self: Arc<Self>, stream: UnixStream) {
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        debug!("client {conn} connected");
        let (read, write) = stream.into_split();
        let writer: Writer = Arc::new(tokio::sync::Mutex::new(write));
        let mut lines = BufReader::new(read).lines();
        let mut background = JoinSet::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            self.dispatch(conn, &writer, line, &mut background).await;
        }
        if self.release_if_owner(conn) {
            info!("client {conn} vanished during capture; cancelling");
        }
        background.abort_all();
        debug!("client {conn} disconnected");
    }

    async fn dispatch(
        self: &Arc<Self>,
        conn: u64,
        w: &Writer,
        line: &str,
        background: &mut JoinSet<()>,
    ) {
        let (id, command, arg) = parse_request(line);
        let id = id.to_string();
        let arg = arg.to_string();

        match command.as_str() {
            "HELLO" => ok(w, &id, &format!("parakeetd {}", env!("CARGO_PKG_VERSION"))).await,
            "STATUS" => {
                let recording = if self.active.lock().is_some() { 1 } else { 0 };
                let payload = format!(
                    "recording={recording} loaded={} languages={}",
                    self.pool.loaded().join(","),
                    self.cfg.languages().join(",")
                );
                ok(w, &id, &payload).await;
            }
            "LOAD" => {
                let daemon = Arc::clone(self);
                let w = Arc::clone(w);
                background.spawn(async move { daemon.load(&w, &id, &arg).await });
            }
            "START" => self.start(conn, w, &id, &arg).await,
            "STOP" => self.stop(conn, w, &id, arg, background).await,
            "CANCEL" => {
                if self.release_if_owner(conn) {
                    ok(w, &id, "").await;
                } else {
                    err(w, &id, "not-recording").await;
                }
            }
            _ => {
                err(
                    w,
                    &id,
                    &format!(
                        "unknown-command {}",
                        if command.is_empty() { line } else { &command }
                    ),
                )
                .await
            }
        }
    }

    async fn load(self: &Arc<Self>, w: &Writer, id: &str, lang: &str) {
        let needed = self.cfg.models_for(lang);
        if needed.is_empty() {
            err(w, id, &format!("unknown-lang {lang}")).await;
            return;
        }
        for model_lang in needed {
            let pool = Arc::clone(&self.pool);
            let l = model_lang.clone();
            match tokio::task::spawn_blocking(move || pool.get(&l)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    error!("loading {model_lang} failed: {e:#}");
                    err(w, id, &format!("load failed: {e:#}")).await;
                    return;
                }
                Err(e) => {
                    err(w, id, &format!("load panicked: {e}")).await;
                    return;
                }
            }
        }
        ok(w, id, "").await;
    }

    fn capture_start(&self) -> Result<Recording> {
        let mut slot = self.capture.lock();
        if slot.is_none() {
            *slot = Some(Capture::spawn(CaptureConfig {
                sample_rate: self.cfg.sample_rate,
                target: self.cfg.target.clone(),
            })?);
        }
        let max_samples = (self.cfg.max_seconds * self.cfg.sample_rate as f32) as usize;
        slot.as_ref()
            .ok_or_else(|| anyhow!("capture unavailable"))?
            .start(max_samples)
    }

    fn capture_stop(&self) {
        if let Some(c) = self.capture.lock().as_ref() {
            c.stop();
        }
    }

    async fn start(self: &Arc<Self>, conn: u64, w: &Writer, id: &str, lang: &str) {
        let needed = self.cfg.models_for(lang);
        if needed.is_empty() {
            err(w, id, &format!("unknown-lang {lang}")).await;
            return;
        }
        // Claim the microphone before the (awaited) start-up so another
        // connection cannot slip in.
        let (preview, preview_stopped) = oneshot::channel();
        let claimed = {
            let mut active = self.active.lock();
            if active.is_some() {
                false
            } else {
                *active = Some(Active {
                    owner: conn,
                    lang: lang.to_string(),
                    recording: None,
                    preview,
                });
                true
            }
        };
        if !claimed {
            err(w, id, "busy").await;
            return;
        }
        let mut recording = match self.capture_start() {
            Ok(r) => r,
            Err(e) => {
                *self.active.lock() = None;
                err(w, id, &format!("{e:#}")).await;
                return;
            }
        };
        // Warm the models while the user is still speaking.
        for model_lang in needed {
            let pool = Arc::clone(&self.pool);
            tokio::task::spawn_blocking(move || {
                if let Err(e) = pool.get(&model_lang) {
                    error!("loading {model_lang} failed: {e:#}");
                }
            });
        }
        // Reply only when samples arrive, so the client's indicator means "live".
        let started = Instant::now();
        if let Err(e) = recording.wait_for_audio(RECORDER_START_TIMEOUT).await {
            self.capture_stop();
            *self.active.lock() = None;
            err(w, id, &format!("{e:#}")).await;
            return;
        }
        debug!("first audio after {} ms", started.elapsed().as_millis());
        let recording = Arc::new(recording);
        let kept = match self.active.lock().as_mut() {
            Some(a) if a.owner == conn => {
                a.recording = Some(Arc::clone(&recording));
                true
            }
            _ => false,
        };
        if !kept {
            self.capture_stop();
            return;
        }
        ok(w, id, "").await;
        if self.cfg.partial_interval_ms > 0 {
            let daemon = Arc::clone(self);
            let (w, id, lang) = (Arc::clone(w), id.to_string(), lang.to_string());
            tokio::spawn(async move {
                daemon
                    .preview(&w, &id, &lang, &recording, preview_stopped)
                    .await
            });
        }
    }

    /// Drops the capture owned by `conn`, if any. Returns whether one existed.
    fn release_if_owner(&self, conn: u64) -> bool {
        let mut active = self.active.lock();
        match active.as_ref() {
            Some(a) if a.owner == conn => {
                *active = None;
                drop(active);
                self.capture_stop();
                true
            }
            _ => false,
        }
    }

    async fn stop(
        self: &Arc<Self>,
        conn: u64,
        w: &Writer,
        id: &str,
        context: String,
        background: &mut JoinSet<()>,
    ) {
        let taken = {
            let mut active = self.active.lock();
            match active.as_ref() {
                Some(a) if a.owner == conn && a.recording.is_some() => active.take(),
                _ => None,
            }
        };
        let Some(Active {
            lang,
            recording: Some(recording),
            preview,
            ..
        }) = taken
        else {
            err(w, id, "not-recording").await;
            return;
        };
        drop(preview);
        self.capture_stop();
        let daemon = Arc::clone(self);
        let w = Arc::clone(w);
        let id = id.to_string();
        background.spawn(async move {
            daemon
                .transcribe(&w, &id, &lang, recording, &context)
                .await
        });
    }

    async fn transcribe(
        self: &Arc<Self>,
        w: &Writer,
        id: &str,
        lang: &str,
        recording: Arc<Recording>,
        context: &str,
    ) {
        let started = Instant::now();
        let pcm = recording.take_pcm();
        let seconds = pcm.len() as f32 / self.cfg.sample_rate as f32;
        let fallback_lang = if lang == AUTO_LANG {
            self.cfg.auto.fallback.clone()
        } else {
            lang.to_string()
        };
        match self.decode(lang, pcm).await {
            Ok(Some((result_lang, text))) => {
                let (text, correction) = self.correct(&result_lang, text, context).await;
                info!(
                    "{lang}: {seconds:.1}s audio -> {result_lang} {} chars in {:.2}s{correction}",
                    text.chars().count(),
                    started.elapsed().as_secs_f32()
                );
                send(w, format!("{id} OK {result_lang} {text}")).await;
            }
            Ok(None) => {
                info!("{lang}: {seconds:.1}s audio -> no speech detected");
                send(w, format!("{id} OK {fallback_lang} ")).await;
            }
            Err(e) => {
                error!("transcribing {lang} failed: {e:#}");
                err(w, id, &format!("transcribe failed: {e:#}")).await;
            }
        }
    }

    /// Returns the text to send and a log suffix describing the correction.
    async fn correct(&self, lang: &str, text: String, context: &str) -> (String, String) {
        let Some(corrector) = self.corrector.clone() else {
            return (text, String::new());
        };
        if lang != "ja" || text.is_empty() {
            return (text, String::new());
        }
        let started = Instant::now();
        let (asr, context) = (text.clone(), context.to_string());
        match tokio::task::spawn_blocking(move || corrector.correct(&asr, &context)).await {
            Ok(corrected) => {
                let corrected = asr::sanitize(&corrected);
                let verdict = if corrected == text { "kept" } else { "changed" };
                let elapsed = started.elapsed().as_secs_f32();
                (corrected, format!(" (correction {verdict} in {elapsed:.2}s)"))
            }
            Err(e) => {
                warn!("correction panicked: {e}");
                (text, String::new())
            }
        }
    }

    /// `Ok(None)` when the capture holds no speech.
    async fn decode(
        self: &Arc<Self>,
        lang: &str,
        pcm: Vec<i16>,
    ) -> Result<Option<(String, String)>> {
        let samples = asr::pcm16_to_f32(&pcm);
        if samples.is_empty() || asr::is_silent(&samples) {
            return Ok(None);
        }
        let samples = match &self.gate {
            Some(gate) => {
                let gate = Arc::clone(gate);
                match tokio::task::spawn_blocking(move || gate.speech(samples))
                    .await
                    .context("VAD task")?
                {
                    Some(s) => s,
                    None => return Ok(None),
                }
            }
            None => samples,
        };
        self.decode_speech(lang, samples).await.map(Some)
    }

    /// Returns the result language and its text.
    async fn decode_speech(&self, lang: &str, samples: Vec<f32>) -> Result<(String, String)> {
        let samples = Arc::new(samples);
        if lang == AUTO_LANG {
            let auto = &self.cfg.auto;
            let (detected, fallback) = tokio::join!(
                self.decode_one(&auto.detector, Arc::clone(&samples)),
                self.decode_one(&auto.fallback, Arc::clone(&samples)),
            );
            let result = asr::decide_auto(&self.cfg, &detected?, &fallback?);
            return Ok((result.lang, result.text));
        }
        let transcript = self.decode_one(lang, samples).await?;
        Ok((lang.to_string(), transcript.text))
    }

    /// Pushes a display-only transcript of the capture until `stopped`'s
    /// sender is dropped.
    async fn preview(
        &self,
        w: &Writer,
        id: &str,
        lang: &str,
        recording: &Recording,
        mut stopped: oneshot::Receiver<()>,
    ) {
        let period = Duration::from_millis(self.cfg.partial_interval_ms);
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        // A decode that outlasts the period skips ticks instead of queueing them.
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut committed = Committed::default();
        let mut shown = String::new();
        loop {
            let tick = async {
                ticks.tick().await;
                self.preview_tick(lang, recording, &mut committed).await
            };
            let text = tokio::select! {
                _ = &mut stopped => return,
                text = tick => text,
            };
            let text = match text {
                Ok(text) if text != shown => text,
                Ok(_) => continue,
                Err(e) => {
                    debug!("preview failed: {e:#}");
                    continue;
                }
            };
            let mut out = w.lock().await;
            // STOP and CANCEL drop the sender before they reply, so checking
            // under the writer lock keeps every PARTIAL ahead of the reply.
            if stopped.try_recv() != Err(oneshot::error::TryRecvError::Empty) {
                return;
            }
            write_line(&mut out, format!("{id} PARTIAL {text}").trim_end().to_string()).await;
            shown = text;
        }
    }

    /// Re-decodes the uncommitted tail; a tail longer than
    /// PREVIEW_TAIL_SECONDS is first committed up to its last pause so the
    /// per-tick cost stays bounded.
    async fn preview_tick(
        &self,
        lang: &str,
        recording: &Recording,
        committed: &mut Committed,
    ) -> Result<String> {
        let started = Instant::now();
        let tail = asr::pcm16_to_f32(&recording.snapshot(committed.samples));
        if asr::is_silent(&tail) {
            return Ok(committed.text.clone());
        }
        let Some(gate) = self.gate.clone() else {
            return Ok(self.decode_speech(lang, tail).await?.1);
        };
        let rate = self.cfg.sample_rate as f32;
        let vad = Arc::clone(&gate);
        let (mut tail, mut segments) = tokio::task::spawn_blocking(move || {
            let segments = vad.segments(&tail);
            (tail, segments)
        })
        .await
        .context("VAD task")?;
        let mut decoded = 0;
        let min_after = (PREVIEW_MIN_AFTER_SECONDS * rate) as usize;
        let cut = (tail.len() as f32 > PREVIEW_TAIL_SECONDS * rate)
            .then(|| asr::last_pause(&segments, tail.len(), min_after))
            .flatten();
        if let Some(cut) = cut {
            let rest = tail.split_off(cut);
            let after = segments.partition_point(|s| s.1 <= cut);
            let rest_segments = segments.split_off(after);
            if let Some(span) = gate.span(tail.len(), &segments) {
                decoded += span.len();
                let (_, text) = self.decode_speech(lang, asr::keep(tail, span)).await?;
                committed.text = asr::join(&committed.text, &text);
            }
            committed.samples += cut;
            tail = rest;
            segments = rest_segments.iter().map(|&(s, e)| (s - cut, e - cut)).collect();
        }
        let captured = committed.samples + tail.len();
        let text = match gate.span(tail.len(), &segments) {
            Some(span) => {
                decoded += span.len();
                self.decode_speech(lang, asr::keep(tail, span)).await?.1
            }
            None => String::new(),
        };
        debug!(
            "preview: {:.1}s captured, {:.1}s committed, {:.1}s decoded in {:.2}s",
            captured as f32 / rate,
            committed.samples as f32 / rate,
            decoded as f32 / rate,
            started.elapsed().as_secs_f32()
        );
        Ok(asr::join(&committed.text, &text))
    }

    async fn decode_one(
        &self,
        lang: &str,
        samples: Arc<Vec<f32>>,
    ) -> Result<crate::sherpa::Transcript> {
        let pool = Arc::clone(&self.pool);
        let lang = lang.to_string();
        let mut transcript =
            tokio::task::spawn_blocking(move || pool.get(&lang)?.transcribe(&samples))
                .await
                .context("decode task")??;
        transcript.text = asr::sanitize(&transcript.text);
        Ok(transcript)
    }
}

/// Preview audio before `samples` is settled as `text` and never decoded again.
#[derive(Default)]
struct Committed {
    samples: usize,
    text: String,
}

/// Returns the listening socket and whether we own the filesystem path.
pub fn listen(path: &Path) -> Result<(UnixListener, bool)> {
    // systemd socket activation hands us the listening socket as fd 3.
    let listen_pid = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());
    let listen_fds = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if listen_pid == Some(std::process::id()) && listen_fds >= 1 {
        info!("using systemd-provided socket");
        // SAFETY: fd 3 is handed to us by systemd and owned by nobody else.
        let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
        std_listener.set_nonblocking(true)?;
        return Ok((UnixListener::from_std(std_listener)?, false));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    info!("listening on {}", path.display());
    Ok((listener, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_carries_context_with_inner_spaces() {
        assert_eq!(
            parse_request("7 stop 会議の 議事録を"),
            ("7", "STOP".to_string(), "会議の 議事録を")
        );
        assert_eq!(parse_request("8 STOP"), ("8", "STOP".to_string(), ""));
    }
}
