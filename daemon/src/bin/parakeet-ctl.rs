//! Diagnostic client for the parakeetd line protocol.

use parakeetd::config;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

struct Client {
    stream: UnixStream,
    /// Reply lines; a reader thread prints PARTIAL events to stderr as they arrive.
    replies: mpsc::Receiver<String>,
    next_id: u64,
}

impl Client {
    fn connect(path: &PathBuf) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("connecting to {}", path.display()))?;
        let reader = BufReader::new(stream.try_clone()?);
        let (tx, replies) = mpsc::channel();
        let connected = Instant::now();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let rest = line.split_once(' ').map_or("", |(_, rest)| rest);
                let (status, text) = rest.split_once(' ').unwrap_or((rest, ""));
                if status == "PARTIAL" {
                    eprintln!("[{:6.2}s] {text}", connected.elapsed().as_secs_f32());
                } else if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            stream,
            replies,
            next_id: 1,
        })
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
            let Ok(line) = self.replies.recv() else {
                bail!("daemon closed the connection");
            };
            let Some((reply_id, rest)) = line.split_once(' ') else {
                continue;
            };
            if reply_id.parse::<u64>() != Ok(id) {
                continue;
            }
            let (status, payload) = rest.split_once(' ').unwrap_or((rest, ""));
            return Ok((status == "OK", payload.to_string()));
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: parakeet-ctl [--socket PATH] <hello|status|load LANG|rec LANG [--seconds N]>"
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
                eprintln!("parakeet-ctl: {e:#}");
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
                    std::thread::sleep(Duration::from_secs_f32(seconds));
                } else {
                    eprint!("recording... press Enter to stop ");
                    std::io::stderr().flush()?;
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                }
                let (ok, result) = client.request("STOP", "")?;
                let text = result
                    .split_once(' ')
                    .map_or("", |(_, text)| text)
                    .to_string();
                (ok, text)
            }
        }
        "-h" | "--help" => {
            println!("usage: parakeet-ctl [--socket PATH] <hello|status|load LANG|rec LANG [--seconds N]>");
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
