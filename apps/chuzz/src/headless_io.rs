//! Reactor-driven descriptor announcements from the headless CLI.

use nagoya::io::{Compat, Read};
use std::time::Duration;

pub async fn read_announcement(stdout: nagoya::process::ChildStdout) -> Result<String, String> {
    nagoya::timeout(Duration::from_secs(60), async move {
        if cfg!(feature = "reactor-witness")
            && std::env::var("CHUZZ_REACTOR_DEFECT").as_deref() == Ok("descriptor-pipe-read")
        {
            return std::future::pending::<Result<String, String>>().await;
        }
        let mut reader = Compat(stdout);
        let mut bytes = Vec::new();
        let mut byte = [0; 1];
        loop {
            if bytes.len() == 4096 {
                return Err("stdout-announce: descriptor line exceeds 4096 bytes".into());
            }
            if reader
                .read(&mut byte)
                .await
                .map_err(|error| format!("stdout-announce: {error}"))?
                == 0
            {
                return Err("stdout-announce: CLI exited before announcing a descriptor".into());
            }
            if byte[0] == b'\n' {
                break;
            }
            bytes.push(byte[0]);
        }
        String::from_utf8(bytes).map_err(|error| format!("stdout-announce: {error}"))
    })
    .await
    .map_err(|_| "stdout-announce: CLI announcement exceeded 60s".to_owned())?
}
