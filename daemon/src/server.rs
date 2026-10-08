//! Unix-socket server speaking a one-line-per-message text protocol.
//!
//! Request:   `<id> <COMMAND> [args]`
//! Response:  `<id> OK [payload]` or `<id> ERR <message>`
//! Event:     `<id> <EVENT> [payload]`, pushed for an earlier request `<id>`
//!
//! Commands:
//!   HELLO            -> `OK voice-jad <version>`
//!   LOAD <lang>      -> `OK` once the model(s) for <lang> are in memory
//!   START <lang> [context]
//!                    -> `OK` once the microphone capture is running. context is
//!                       the text before the cursor, used to correct Japanese
//!                       homophones. Until the capture ends, events with the
//!                       START id follow:
//!                       `PARTIAL <text>` is a provisional transcript of the
//!                       audio not yet committed, sent whenever it changes.
//!                       `COMMIT <lang> <text>` is final text for the client to
//!                       enter right away: once commit_after_seconds of audio
//!                       are uncommitted, the part up to the longest pause in
//!                       their second half (or, past max_seconds, the quietest
//!                       moment) is transcribed like STOP while the capture
//!                       continues.
//!                       `ENDED` means the capture ended itself on reaching
//!                       max_recording_seconds or losing its PipeWire
//!                       connection; its rest came as the last COMMIT.
//!                       No event follows ENDED or the STOP/CANCEL reply.
//!   STOP [context]   -> `OK <lang> <text>` for the audio not yet committed (text
//!                       may be empty), after every COMMIT. context replaces
//!                       START's when nothing was committed yet
//!   CANCEL           -> `OK`; drops the audio not yet committed
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
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Interval, MissedTickBehavior};

use crate::asr::{self, Gate, Pool, SegmentLimits};
use crate::audio::{Capture, CaptureConfig, Recording};
use crate::config::{Config, AUTO_LANG};
use crate::correct::Corrector;
use crate::sherpa::Transcript;

/// Seconds to wait for the first samples before declaring the microphone dead.
const RECORDER_START_TIMEOUT: Duration = Duration::from_secs(3);
/// The preview re-decodes its tail every tick; past this it settles up to a pause.
const PREVIEW_TAIL_SECONDS: f32 = 10.0;
/// How often a capture checks for a segment to commit when the preview is off.
const CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// How the owner ends a capture; the capture's session sends the reply.
enum End {
    Stop { id: String, context: String },
    Cancel { id: String },
}

struct Active {
    owner: u64,
    session: u64,
    /// Dropping it ends the session without a reply.
    end: oneshot::Sender<End>,
}

pub struct Daemon {
    cfg: Config,
    pool: Arc<Pool>,
    gate: Option<Arc<Gate>>,
    corrector: Option<Arc<Corrector>>,
    capture: Mutex<Option<Capture>>,
    active: Mutex<Option<Active>>,
    next_conn: AtomicU64,
    next_session: AtomicU64,
    limits: SegmentLimits,
}

type Writer = Arc<tokio::sync::Mutex<OwnedWriteHalf>>;

async fn send(w: &Writer, line: String) {
    if let Err(e) = w.lock().await.write_all(format!("{line}\n").as_bytes()).await {
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
        let corrector = cfg
            .correction
            .clone()
            .filter(|_| crate::correct::cpu_supported())
            .map(|c| Arc::new(Corrector::new(c)));
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
            limits: SegmentLimits::new(&cfg),
            cfg,
            pool,
            gate,
            corrector,
            capture: Mutex::new(capture),
            active: Mutex::new(None),
            next_conn: AtomicU64::new(1),
            next_session: AtomicU64::new(1),
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
        if self.release(conn).is_some() {
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
            "HELLO" => ok(w, &id, &format!("voice-jad {}", env!("CARGO_PKG_VERSION"))).await,
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
            "START" => self.start(conn, w, &id, &arg, background).await,
            "STOP" => {
                let end = End::Stop {
                    id: id.clone(),
                    context: arg,
                };
                self.end_capture(conn, w, &id, end).await
            }
            "CANCEL" => {
                let end = End::Cancel { id: id.clone() };
                self.end_capture(conn, w, &id, end).await
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
        if !slot.as_ref().is_some_and(Capture::alive) {
            *slot = Some(Capture::spawn(CaptureConfig {
                sample_rate: self.cfg.sample_rate,
                target: self.cfg.target.clone(),
            })?);
        }
        let max_samples = (self.cfg.max_recording_seconds * self.cfg.sample_rate as f32) as usize;
        slot.as_ref()
            .ok_or_else(|| anyhow!("capture unavailable"))?
            .start(max_samples)
    }

    fn capture_stop(&self) {
        if let Some(c) = self.capture.lock().as_ref() {
            c.stop();
        }
    }

    async fn start(
        self: &Arc<Self>,
        conn: u64,
        w: &Writer,
        id: &str,
        arg: &str,
        background: &mut JoinSet<()>,
    ) {
        let (lang, context) = arg.split_once(' ').unwrap_or((arg, ""));
        let needed = self.cfg.models_for(lang);
        if needed.is_empty() {
            err(w, id, &format!("unknown-lang {lang}")).await;
            return;
        }
        // Claim the microphone before the (awaited) start-up so another
        // connection cannot slip in.
        let session = self.next_session.fetch_add(1, Ordering::Relaxed);
        let (end, end_rx) = oneshot::channel();
        let claimed = {
            let mut active = self.active.lock();
            if active.is_some() {
                false
            } else {
                *active = Some(Active {
                    owner: conn,
                    session,
                    end,
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
        if !self.active.lock().as_ref().is_some_and(|a| a.session == session) {
            self.capture_stop();
            return;
        }
        ok(w, id, "").await;
        let s = Session {
            id: session,
            start_id: id.to_string(),
            lang: lang.to_string(),
            start_context: context.to_string(),
            w: Arc::clone(w),
            recording,
            committed: String::new(),
            drained: 0,
            segmenting: true,
        };
        let daemon = Arc::clone(self);
        background.spawn(async move { daemon.run(s, end_rx).await });
    }

    /// Takes the capture `conn` owns and stops its audio. Stopping under the
    /// lock keeps another connection's START from being stopped instead.
    fn release(&self, conn: u64) -> Option<Active> {
        let mut active = self.active.lock();
        let released = active.take_if(|a| a.owner == conn)?;
        self.capture_stop();
        Some(released)
    }

    /// Hands STOP or CANCEL to the session of the capture `conn` owns.
    async fn end_capture(&self, conn: u64, w: &Writer, id: &str, end: End) {
        let sent = self.release(conn).is_some_and(|a| a.end.send(end).is_ok());
        if !sent {
            err(w, id, "not-recording").await;
        }
    }

    async fn run(self: Arc<Self>, mut s: Session, mut end_rx: oneshot::Receiver<End>) {
        let period = match self.cfg.partial_interval_ms {
            0 => CHECK_INTERVAL,
            ms => Duration::from_millis(ms),
        };
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        // A decode that outlasts the period skips ticks instead of queueing them.
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut preview = Settled::default();
        let mut shown = String::new();
        let mut segment: Option<Segment> = None;
        let end = loop {
            let idle = segment.is_none();
            tokio::select! {
                biased;
                end = &mut end_rx => match end {
                    Ok(end) => break Some(end),
                    Err(_) => return,
                },
                result = in_flight(&mut segment) => {
                    if let Some(done) = segment.take() {
                        self.commit(&mut s, done.samples, result).await;
                    }
                    preview = Settled::default();
                    shown.clear();
                }
                step = self.tick(&s, &mut ticks, &mut preview, idle) => match step {
                    Step::Idle => {}
                    Step::Partial(text) if text != shown => {
                        send(&s.w, format!("{} PARTIAL {text}", s.start_id).trim_end().to_string())
                            .await;
                        shown = text;
                    }
                    Step::Partial(_) => {}
                    Step::Segment(started) => segment = Some(started),
                    Step::End => break None,
                },
            }
        };
        if let Some(End::Cancel { id }) = &end {
            ok(&s.w, &id, "").await;
            return;
        }
        if end.is_none() {
            let active = self.active.lock();
            if active.as_ref().is_some_and(|a| a.session == s.id) {
                self.capture_stop();
            }
        }
        if let Some(mut done) = segment.take() {
            let result = in_flight_result(&mut done).await;
            self.commit(&mut s, done.samples, result).await;
        }
        let context = match &end {
            Some(End::Stop { context, .. }) if s.committed.is_empty() => context.clone(),
            _ => s.context(),
        };
        let pcm = s.recording.take_pcm();
        let result = self.finalize(&s.lang, pcm, &context, s.drained > 0).await;
        let end = match end {
            Some(end) => end,
            None => {
                let ours = self.active.lock().take_if(|a| a.session == s.id).is_some();
                if ours {
                    match result {
                        Ok(Some((lang, text))) if !text.is_empty() => {
                            send(&s.w, format!("{} COMMIT {lang} {text}", s.start_id)).await
                        }
                        Ok(_) => {}
                        Err(e) => error!("transcribing {} failed: {e:#}", s.lang),
                    }
                    send(&s.w, format!("{} ENDED", s.start_id)).await;
                    return;
                }
                // STOP or CANCEL took the capture while it was being finished.
                match end_rx.try_recv() {
                    Ok(end) => end,
                    Err(_) => return,
                }
            }
        };
        let id = match end {
            End::Cancel { id } => return ok(&s.w, &id, "").await,
            End::Stop { id, .. } => id,
        };
        match result {
            Ok(Some((lang, text))) => send(&s.w, format!("{id} OK {lang} {text}")).await,
            Ok(None) => {
                let fallback = if s.lang == AUTO_LANG {
                    &self.cfg.auto.fallback
                } else {
                    &s.lang
                };
                send(&s.w, format!("{id} OK {fallback} ")).await
            }
            Err(e) => {
                error!("transcribing {} failed: {e:#}", s.lang);
                err(&s.w, &id, &format!("transcribe failed: {e:#}")).await
            }
        }
    }

    async fn tick(
        self: &Arc<Self>,
        s: &Session,
        ticks: &mut Interval,
        preview: &mut Settled,
        idle: bool,
    ) -> Step {
        ticks.tick().await;
        if s.recording.lost() {
            warn!("microphone connection lost; ending the capture");
            return Step::End;
        }
        let len = s.recording.len();
        let rate = self.cfg.sample_rate as f32;
        if (s.drained + len) as f32 >= self.cfg.max_recording_seconds * rate {
            info!(
                "capture reached max_recording_seconds ({:.0}s); ending it",
                self.cfg.max_recording_seconds
            );
            return Step::End;
        }
        if idle && s.segmenting && len >= self.limits.commit_after {
            match self.start_segment(s).await {
                Ok(Some(segment)) => return Step::Segment(segment),
                Ok(None) => {}
                Err(e) => debug!("segment search failed: {e:#}"),
            }
        }
        // A preview decode running beside a segment's decode and correction
        // oversubscribes the CPU: llama.cpp's spinning threads then took 6.6 s
        // instead of 0.4 s to correct a 57 s segment in `auto`.
        if self.cfg.partial_interval_ms == 0 || !idle {
            return Step::Idle;
        }
        match self.preview_tick(&s.lang, &s.recording, preview).await {
            Ok(text) => Step::Partial(text),
            Err(e) => {
                debug!("preview failed: {e:#}");
                Step::Idle
            }
        }
    }

    /// Starts transcribing the head of the uncommitted audio if it has a cut.
    async fn start_segment(self: &Arc<Self>, s: &Session) -> Result<Option<Segment>> {
        let mut pcm = s.recording.snapshot(0);
        let samples = asr::pcm16_to_f32(&pcm);
        let gate = self.gate.clone();
        let (samples, speech) = tokio::task::spawn_blocking(move || {
            let speech = gate.map(|gate| gate.segments(&samples));
            (samples, speech)
        })
        .await
        .context("VAD task")?;
        let Some(cut) = asr::segment_cut(&samples, speech.as_deref(), &self.limits) else {
            return Ok(None);
        };
        let rate = self.cfg.sample_rate as f32;
        info!(
            "committing {:.1}s to {:.1}s of the capture, {:.1}s uncommitted",
            s.drained as f32 / rate,
            (s.drained + cut) as f32 / rate,
            samples.len() as f32 / rate
        );
        pcm.truncate(cut);
        let daemon = Arc::clone(self);
        let (lang, context, after_cut) = (s.lang.clone(), s.context(), s.drained > 0);
        let task = tokio::spawn(async move {
            daemon.finalize(&lang, pcm, &context, after_cut).await
        });
        Ok(Some(Segment { samples: cut, task }))
    }

    /// Pushes a transcribed segment and forgets its audio. A failed segment
    /// keeps its audio for STOP and ends segmenting.
    async fn commit(&self, s: &mut Session, samples: usize, result: Result<Option<Decoded>>) {
        match result {
            Ok(Some((lang, text))) => {
                if !text.is_empty() {
                    send(&s.w, format!("{} COMMIT {lang} {text}", s.start_id)).await;
                    s.committed.push_str(&text);
                }
            }
            Ok(None) => {}
            Err(e) => {
                error!("transcribing a {} segment failed: {e:#}", s.lang);
                s.segmenting = false;
                return;
            }
        }
        s.recording.drain(samples);
        s.drained += samples;
    }

    /// Transcribes like STOP: the whole speech span, the `auto` decision and
    /// homophone correction. `after_cut` marks audio that starts at a segment
    /// cut. `Ok(None)` when the audio holds no speech.
    async fn finalize(
        self: &Arc<Self>,
        lang: &str,
        pcm: Vec<i16>,
        context: &str,
        after_cut: bool,
    ) -> Result<Option<Decoded>> {
        let started = Instant::now();
        let seconds = pcm.len() as f32 / self.cfg.sample_rate as f32;
        let Some((result_lang, transcript)) = self.decode(lang, pcm, after_cut).await? else {
            info!("{lang}: {seconds:.1}s audio -> no speech detected");
            return Ok(None);
        };
        let text = punctuate(&result_lang, transcript).await;
        let (text, correction) = self.correct(&result_lang, text, context).await;
        info!(
            "{lang}: {seconds:.1}s audio -> {result_lang} {} chars in {:.2}s{correction}",
            text.chars().count(),
            started.elapsed().as_secs_f32()
        );
        Ok(Some((result_lang, text)))
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
        after_cut: bool,
    ) -> Result<Option<(String, Transcript)>> {
        let samples = asr::pcm16_to_f32(&pcm);
        if samples.is_empty() || asr::is_silent(&samples) {
            return Ok(None);
        }
        let samples = match &self.gate {
            Some(gate) => {
                let gate = Arc::clone(gate);
                match tokio::task::spawn_blocking(move || gate.speech(samples, after_cut))
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

    /// Returns the result language and its transcript.
    async fn decode_speech(&self, lang: &str, samples: Vec<f32>) -> Result<(String, Transcript)> {
        let samples = Arc::new(samples);
        if lang == AUTO_LANG {
            let auto = &self.cfg.auto;
            let (detected, fallback) = tokio::join!(
                self.decode_one(&auto.detector, Arc::clone(&samples)),
                self.decode_one(&auto.fallback, Arc::clone(&samples)),
            );
            let result = asr::decide_auto(&self.cfg, detected?, fallback?);
            return Ok((result.lang, result.transcript));
        }
        let transcript = self.decode_one(lang, samples).await?;
        Ok((lang.to_string(), transcript))
    }

    /// Re-decodes the unsettled tail; a tail longer than
    /// PREVIEW_TAIL_SECONDS is first settled up to its last pause so the
    /// per-tick cost stays bounded.
    async fn preview_tick(
        &self,
        lang: &str,
        recording: &Recording,
        settled: &mut Settled,
    ) -> Result<String> {
        let started = Instant::now();
        let tail = asr::pcm16_to_f32(&recording.snapshot(settled.samples));
        if asr::is_silent(&tail) {
            return Ok(settled.text.clone());
        }
        let Some(gate) = self.gate.clone() else {
            return Ok(self.decode_speech(lang, tail).await?.1.text);
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
        let min_after = self.limits.min_after;
        let cut = (tail.len() as f32 > PREVIEW_TAIL_SECONDS * rate)
            .then(|| asr::last_pause(&segments, tail.len(), min_after))
            .flatten();
        if let Some(cut) = cut {
            let rest = tail.split_off(cut);
            let after = segments.partition_point(|s| s.1 <= cut);
            let rest_segments = segments.split_off(after);
            if let Some(span) = gate.span(tail.len(), &segments) {
                decoded += span.len();
                let text = self.decode_speech(lang, asr::keep(tail, span)).await?.1.text;
                settled.text = asr::join(&settled.text, &text);
            }
            settled.samples += cut;
            tail = rest;
            segments = rest_segments.iter().map(|&(s, e)| (s - cut, e - cut)).collect();
        }
        let captured = settled.samples + tail.len();
        let text = match gate.span(tail.len(), &segments) {
            Some(span) => {
                decoded += span.len();
                self.decode_speech(lang, asr::keep(tail, span)).await?.1.text
            }
            None => String::new(),
        };
        debug!(
            "preview: {:.1}s captured, {:.1}s settled, {:.1}s decoded in {:.2}s",
            captured as f32 / rate,
            settled.samples as f32 / rate,
            decoded as f32 / rate,
            started.elapsed().as_secs_f32()
        );
        Ok(asr::join(&settled.text, &text))
    }

    async fn decode_one(
        &self,
        lang: &str,
        samples: Arc<Vec<f32>>,
    ) -> Result<Transcript> {
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

/// Marks pauses and the end of a Japanese transcript; any other is kept.
async fn punctuate(lang: &str, transcript: Transcript) -> String {
    if lang != "ja" || transcript.text.is_empty() {
        return transcript.text;
    }
    let text = transcript.text.clone();
    match tokio::task::spawn_blocking(move || crate::punct::punctuate(&transcript)).await {
        Ok(Ok(punctuated)) => punctuated,
        Ok(Err(e)) => {
            warn!("punctuation skipped: {e:#}");
            text
        }
        Err(e) => {
            warn!("punctuation panicked: {e}");
            text
        }
    }
}

/// Preview audio before `samples` is settled as `text` and never decoded again.
#[derive(Default)]
struct Settled {
    samples: usize,
    text: String,
}

/// Result language and text.
type Decoded = (String, String);

/// One capture, from the START reply until its end.
struct Session {
    id: u64,
    /// The START request id its events carry.
    start_id: String,
    lang: String,
    start_context: String,
    w: Writer,
    /// Holds the audio not yet committed.
    recording: Recording,
    /// Text pushed as COMMIT so far.
    committed: String,
    /// Samples dropped from the front of `recording` by commits.
    drained: usize,
    segmenting: bool,
}

impl Session {
    /// Text before the audio not yet committed, for homophone correction.
    fn context(&self) -> String {
        format!("{}{}", self.start_context, self.committed)
    }
}

/// The head of the recording, transcribed while the capture continues.
struct Segment {
    samples: usize,
    task: JoinHandle<Result<Option<Decoded>>>,
}

impl Drop for Segment {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn in_flight_result(segment: &mut Segment) -> Result<Option<Decoded>> {
    (&mut segment.task)
        .await
        .unwrap_or_else(|e| Err(anyhow!("segment task: {e}")))
}

/// Never resolves without a segment.
async fn in_flight(segment: &mut Option<Segment>) -> Result<Option<Decoded>> {
    match segment {
        Some(segment) => in_flight_result(segment).await,
        None => std::future::pending().await,
    }
}

enum Step {
    Idle,
    Partial(String),
    Segment(Segment),
    End,
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
