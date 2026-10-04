//! Diagnostic client for the voice-jad line protocol.

use voice_jad::{asr, config};

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

enum Msg {
    Line(String),
    Enter,
}

struct Client {
    stream: UnixStream,
    /// Daemon lines except PARTIAL; a reader thread prints events to stderr
    /// as they arrive.
    rx: mpsc::Receiver<Msg>,
    tx: mpsc::Sender<Msg>,
    next_id: u64,
    /// Texts of the COMMIT events seen so far.
    committed: Vec<String>,
    ended: bool,
}

impl Client {
    fn connect(path: &PathBuf) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("connecting to {}", path.display()))?;
        let reader = BufReader::new(stream.try_clone()?);
        let (tx, rx) = mpsc::channel();
        let lines = tx.clone();
        let connected = Instant::now();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let rest = line.split_once(' ').map_or("", |(_, rest)| rest);
                let (status, text) = rest.split_once(' ').unwrap_or((rest, ""));
                if matches!(status, "PARTIAL" | "COMMIT" | "ENDED") {
                    let event = if status == "PARTIAL" { "" } else { status };
                    eprintln!("[{:6.2}s] {event} {text}", connected.elapsed().as_secs_f32());
                }
                if status != "PARTIAL" && lines.send(Msg::Line(line)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            stream,
            rx,
            tx,
            next_id: 1,
            committed: Vec::new(),
            ended: false,
        })
    }

    /// Records COMMIT and ENDED events; returns `(id, ok, payload)` for a reply.
    fn note(&mut self, line: &str) -> Option<(u64, bool, String)> {
        let (id, rest) = line.split_once(' ')?;
        let (status, payload) = rest.split_once(' ').unwrap_or((rest, ""));
        match status {
            "OK" | "ERR" => Some((id.parse().ok()?, status == "OK", payload.to_string())),
            "COMMIT" => {
                let text = payload.split_once(' ').map_or("", |(_, text)| text);
                self.committed.push(text.to_string());
                None
            }
            "ENDED" => {
                self.ended = true;
                None
            }
            _ => None,
        }
    }

    fn recv(&self, deadline: Option<Instant>) -> Result<Option<Msg>> {
        let Some(deadline) = deadline else {
            return Ok(Some(self.rx.recv().context("daemon closed the connection")?));
        };
        match self.rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(msg) => Ok(Some(msg)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("daemon closed the connection"),
        }
    }

    fn request(&mut self, command: &str, arg: &str) -> Result<(bool, String)> {
        let id = self.next_id;
        self.next_id += 1;
        let suffix = if arg.is_empty() {
            String::new()
        } else {
            format!(" {arg}")
        };
        writeln!(self.stream, "{id} {command}{suffix}")?;
        self.stream.flush()?;

        loop {
            if let Some(Msg::Line(line)) = self.recv(None)? {
                if let Some((reply_id, ok, payload)) = self.note(&line) {
                    if reply_id == id {
                        return Ok((ok, payload));
                    }
                }
            }
        }
    }

    /// Lets the capture run for `seconds`, or until Enter; returns early when
    /// the daemon ends it.
    fn record(&mut self, seconds: Option<f32>) -> Result<()> {
        let deadline = seconds.map(|s| Instant::now() + Duration::from_secs_f32(s));
        if deadline.is_none() {
            let enter = self.tx.clone();
            std::thread::spawn(move || {
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);
                let _ = enter.send(Msg::Enter);
            });
        }
        while !self.ended {
            match self.recv(deadline)? {
                Some(Msg::Line(line)) => {
                    self.note(&line);
                }
                Some(Msg::Enter) | None => break,
            }
        }
        Ok(())
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: voice-ja-ctl [--socket PATH] <hello|status|load LANG|rec LANG [--seconds N]>"
    );
    std::process::exit(2);
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let mut socket = None;
    if args.peek().map(String::as_str) == Some("--socket") {
        args.next();
        socket = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())));
    }
    let command = args.next().unwrap_or_else(|| usage());
    let path = socket.unwrap_or_else(|| {
        config::load(None)
            .map(|c| c.socket_path)
            .unwrap_or_else(|e| {
                eprintln!("voice-ja-ctl: {e:#}");
                std::process::exit(1);
            })
    });
    let mut client = Client::connect(&path)?;

    let (ok, mut payload) = match command.as_str() {
        "hello" => client.request("HELLO", "")?,
        "status" => client.request("STATUS", "")?,
        "load" => {
            let lang = args.next().unwrap_or_else(|| usage());
            let started = Instant::now();
            let (ok, payload) = client.request("LOAD", &lang)?;
            (
                ok,
                if ok && payload.is_empty() {
                    format!("loaded in {:.1}s", started.elapsed().as_secs_f32())
                } else {
                    payload
                },
            )
        }
        "rec" => {
            let lang = args.next().unwrap_or_else(|| usage());
            let mut seconds = None;
            while let Some(arg) = args.next() {
                if arg != "--seconds" {
                    usage();
                }
                seconds = Some(
                    args.next()
                        .unwrap_or_else(|| usage())
                        .parse::<f32>()
                        .context("invalid --seconds")?,
                );
            }
            let (ok, payload) = client.request("START", &lang)?;
            if !ok {
                (ok, payload)
            } else {
                if let Some(seconds) = seconds {
                    eprintln!("recording {seconds:.1}s...");
                } else {
                    eprint!("recording... press Enter to stop ");
                    std::io::stderr().flush()?;
                }
                client.record(seconds)?;
                let (ok, rest) = if client.ended {
                    (true, String::new())
                } else {
                    let (ok, result) = client.request("STOP", "")?;
                    let text = result.split_once(' ').map_or("", |(_, text)| text);
                    (ok, if ok { text.to_string() } else { result })
                };
                if ok {
                    let text = client.committed.iter().chain([&rest]).fold(String::new(), |all, part| {
                        asr::join(&all, part)
                    });
                    (ok, text)
                } else {
                    (ok, rest)
                }
            }
        }
        "-h" | "--help" => {
            println!("usage: voice-ja-ctl [--socket PATH] <hello|status|load LANG|rec LANG [--seconds N]>");
            return Ok(());
        }
        _ => usage(),
    };

    if ok {
        println!("{payload}");
    } else {
        if payload.is_empty() {
            payload = "request failed".into();
        }
        eprintln!("ERR {payload}");
        std::process::exit(1);
    }
    Ok(())
}
