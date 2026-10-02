//! Microphone capture through a persistent PipeWire stream.
//!
//! The stream is connected once at start-up and left inactive, so no audio
//! flows until a capture starts. Activating an already-negotiated stream keeps
//! start latency to a few milliseconds.

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use log::{debug, info, warn};
use parking_lot::Mutex;
use pipewire as pw;
use pw::{properties::properties, spa};
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub sample_rate: i32,
    /// PipeWire `target.object` (node name or serial); None = default source.
    pub target: Option<String>,
}

enum Cmd {
    SetActive(bool),
    Quit,
}

#[derive(Default)]
struct Shared {
    sink: Mutex<Option<Arc<CaptureSink>>>,
}

struct CaptureSink {
    pcm: Mutex<Vec<i16>>,
    first: Mutex<Option<oneshot::Sender<()>>>,
    max_samples: usize,
}

pub struct Capture {
    cmd: pw::channel::Sender<Cmd>,
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

/// One bounded capture session.
pub struct Recording {
    sink: Arc<CaptureSink>,
    first: Option<oneshot::Receiver<()>>,
}

impl Recording {
    /// Waits for the first samples so the caller can tell the user the
    /// microphone is really live.
    pub async fn wait_for_audio(&mut self, timeout: Duration) -> Result<()> {
        let first = self
            .first
            .take()
            .ok_or_else(|| anyhow!("capture start already awaited"))?;
        match tokio::time::timeout(timeout, first).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => bail!("capture stream closed before delivering audio"),
            Err(_) => bail!(
                "microphone produced no audio within {:.0}s",
                timeout.as_secs_f32()
            ),
        }
    }

    /// Copy of the samples captured so far, from `from` on; capture continues.
    pub fn snapshot(&self, from: usize) -> Vec<i16> {
        self.sink.pcm.lock().get(from..).map_or_else(Vec::new, <[i16]>::to_vec)
    }

    pub fn take_pcm(&self) -> Vec<i16> {
        std::mem::take(&mut *self.sink.pcm.lock())
    }
}

impl Capture {
    /// Connects the stream and blocks until PipeWire has negotiated it.
    pub fn spawn(cfg: CaptureConfig) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let (cmd_tx, cmd_rx) = pw::channel::channel::<Cmd>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(), String>>();
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("pipewire".into())
            .spawn(move || {
                if let Err(e) = run_loop(cfg, cmd_rx, thread_shared, ready_tx.clone()) {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
            })
            .context("spawning the PipeWire thread")?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                cmd: cmd_tx,
                shared,
                thread: Some(thread),
            }),
            Ok(Err(e)) => bail!("PipeWire capture stream failed: {e}"),
            Err(_) => bail!("PipeWire capture stream did not become ready within 10s"),
        }
    }

    /// Starts delivering PCM; the stream is activated on the PipeWire thread.
    pub fn start(&self, max_samples: usize) -> Result<Recording> {
        let (first_tx, first_rx) = oneshot::channel();
        let sink = Arc::new(CaptureSink {
            pcm: Mutex::new(Vec::with_capacity(max_samples)),
            first: Mutex::new(Some(first_tx)),
            max_samples,
        });
        *self.shared.sink.lock() = Some(Arc::clone(&sink));
        if self.cmd.send(Cmd::SetActive(true)).is_err() {
            *self.shared.sink.lock() = None;
            bail!("PipeWire thread is gone");
        }
        Ok(Recording {
            sink,
            first: Some(first_rx),
        })
    }

    /// Stops delivering PCM and deactivates the stream.
    pub fn stop(&self) {
        *self.shared.sink.lock() = None;
        if self.cmd.send(Cmd::SetActive(false)).is_err() {
            warn!("PipeWire thread is gone; cannot deactivate stream");
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct UserData {
    shared: Arc<Shared>,
    ready: Option<std_mpsc::Sender<Result<(), String>>>,
    first_chunk_logged: bool,
}

fn run_loop(
    cfg: CaptureConfig,
    cmd_rx: pw::channel::Receiver<Cmd>,
    shared: Arc<Shared>,
    ready: std_mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("pw_main_loop_new")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("pw_context_new")?;
    let core = context.connect_rc(None).context("connecting to PipeWire")?;

    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => "parakeetd",
        *pw::keys::APP_NAME => "parakeetd",
        // 20 ms quantum keeps stop latency and buffer churn low.
        *pw::keys::NODE_LATENCY => format!("{}/{}", cfg.sample_rate / 50, cfg.sample_rate),
    };
    if let Some(target) = &cfg.target {
        props.insert("target.object", target.as_str());
    }

    let stream =
        pw::stream::StreamRc::new(core.clone(), "parakeetd", props).context("pw_stream_new")?;
    let user_data = UserData {
        shared,
        ready: Some(ready),
        first_chunk_logged: false,
    };

    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .state_changed(|_, ud, old, new| {
            debug!("stream state {old:?} -> {new:?}");
            match &new {
                pw::stream::StreamState::Paused => {
                    if let Some(tx) = ud.ready.take() {
                        let _ = tx.send(Ok(()));
                    }
                }
                pw::stream::StreamState::Error(msg) => {
                    warn!("PipeWire stream error: {msg}");
                    if let Some(tx) = ud.ready.take() {
                        let _ = tx.send(Err(msg.clone()));
                    }
                }
                pw::stream::StreamState::Streaming => ud.first_chunk_logged = false,
                _ => {}
            }
        })
        .process(|stream, ud| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let size = data.chunk().size() as usize;
            let offset = data.chunk().offset() as usize;
            let Some(bytes) = data.data() else {
                return;
            };
            let end = (offset + size).min(bytes.len());
            let sink_guard = ud.shared.sink.lock();
            let Some(sink) = sink_guard.as_ref() else {
                return;
            };
            let mut pcm = sink.pcm.lock();
            let available = sink.max_samples.saturating_sub(pcm.len());
            if available == 0 {
                return;
            }
            let before = pcm.len();
            pcm.extend(
                bytes[offset..end]
                    .chunks_exact(2)
                    .take(available)
                    .map(|b| i16::from_le_bytes([b[0], b[1]])),
            );
            if pcm.len() > before {
                if !ud.first_chunk_logged {
                    debug!("first audio chunk: {} samples", pcm.len() - before);
                    ud.first_chunk_logged = true;
                }
                if let Some(tx) = sink.first.lock().take() {
                    let _ = tx.send(());
                }
            }
        })
        .register()
        .context("registering stream listener")?;

    // Ask for 16 kHz mono S16; PipeWire's stream adapter converts and resamples
    // from whatever the source provides.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::S16LE);
    audio_info.set_rate(cfg.sample_rate as u32);
    audio_info.set_channels(1);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|e| anyhow!("serialising format pod: {e:?}"))?
    .0
    .into_inner();
    let mut params =
        [spa::pod::Pod::from_bytes(&values).ok_or_else(|| anyhow!("invalid format pod"))?];

    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS
                | pw::stream::StreamFlags::INACTIVE,
            &mut params,
        )
        .context("connecting capture stream")?;
    info!(
        "PipeWire capture stream connected ({} Hz mono, target {})",
        cfg.sample_rate,
        cfg.target.as_deref().unwrap_or("default source")
    );

    let loop_stream = stream.clone();
    let loop_main = mainloop.clone();
    let _receiver = cmd_rx.attach(mainloop.loop_(), move |cmd| match cmd {
        Cmd::SetActive(active) => {
            if let Err(e) = loop_stream.set_active(active) {
                warn!("pw_stream_set_active({active}) failed: {e}");
            }
        }
        Cmd::Quit => loop_main.quit(),
    });

    mainloop.run();
    Ok(())
}
