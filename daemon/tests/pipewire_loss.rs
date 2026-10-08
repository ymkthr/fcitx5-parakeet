//! Losing the PipeWire server must end every capture connected to it. One
//! still waiting for its first samples closes instead of running out its start
//! timeout, and each refuses to start again. A capture spawned once the server
//! is back must connect to it.

use std::env;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{self, Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use voice_jad::audio::{Capture, CaptureConfig};

/// A `pipewire` daemon listening only in its own runtime directory.
struct PrivateServer {
    runtime_dir: PathBuf,
    daemon: Child,
}

impl PrivateServer {
    fn start() -> Self {
        let runtime_dir = env::temp_dir().join(format!("voice-jad-{}-pipewire", process::id()));
        fs::create_dir_all(&runtime_dir).expect("creating the runtime directory");
        // PIPEWIRE_NO_CONFIG keeps the vendor config free of host overrides, which could
        // rename the socket or start a session manager that links a real microphone.
        let daemon = Command::new("pipewire")
            .env("PIPEWIRE_RUNTIME_DIR", &runtime_dir)
            .env("PIPEWIRE_NO_CONFIG", "true")
            .spawn()
            .unwrap_or_else(|e| {
                let _ = fs::remove_dir_all(&runtime_dir);
                panic!("running pipewire: {e}")
            });
        let mut server = Self {
            runtime_dir,
            daemon,
        };
        let socket = server.runtime_dir.join("pipewire-0");
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(&socket).is_err() {
            if let Some(status) = server.daemon.try_wait().expect("polling pipewire") {
                panic!("pipewire exited with {status} before listening");
            }
            assert!(
                Instant::now() < deadline,
                "pipewire is not listening on {} after 5s",
                socket.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
        server
    }

    fn restart(self) -> Self {
        drop(self);
        Self::start()
    }
}

impl Drop for PrivateServer {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = fs::remove_dir_all(&self.runtime_dir);
    }
}

#[tokio::test]
#[ignore = "needs the pipewire binary"]
async fn captures_die_with_pipewire_and_spawn_again_after_restart() {
    let mut server = PrivateServer::start();
    // Capture connects to the default remote, which libpipewire looks up in the environment.
    env::set_var("PIPEWIRE_RUNTIME_DIR", &server.runtime_dir);
    env::remove_var("PIPEWIRE_REMOTE");
    let config = CaptureConfig {
        sample_rate: 16_000,
        target: None,
    };
    let capture = Capture::spawn(config.clone()).expect("capture stream on the private server");
    let idle = Capture::spawn(config.clone()).expect("idle stream on the private server");
    let mut recording = capture.start(16_000).expect("starting a capture");

    server.daemon.kill().expect("killing pipewire");
    server.daemon.wait().expect("reaping pipewire");

    let err = recording
        .wait_for_audio(Duration::from_secs(5))
        .await
        .expect_err("the private server has no source to deliver audio");
    assert_eq!(
        format!("{err:#}"),
        "capture stream closed before delivering audio",
        "losing the server must close the capture, not run out its timeout"
    );
    assert!(
        recording.lost(),
        "the recording must report the lost connection"
    );
    for (name, dead) in [("capturing", &capture), ("idle", &idle)] {
        let deadline = Instant::now() + Duration::from_secs(5);
        while dead.alive() {
            assert!(
                Instant::now() < deadline,
                "the {name} capture still looks alive 5s after its server died"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            dead.start(16_000).is_err(),
            "the {name} capture must refuse to start without its server"
        );
    }

    let _server = server.restart();
    let fresh = Capture::spawn(config).expect("capture stream on the restarted server");
    assert!(
        fresh.alive(),
        "a capture spawned after the restart must be alive"
    );
}
