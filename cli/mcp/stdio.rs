use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Result;

use super::TursoMcpServer;

pub(super) fn run(server: &TursoMcpServer) -> Result<()> {
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(io::stdin());
        for line in reader.lines() {
            let failed = line.is_err();
            if tx.send(line).is_err() || failed {
                break;
            }
        }
    });

    loop {
        if server.interrupted() {
            eprintln!("MCP server interrupted, shutting down...");
            break;
        }

        // The timeout is what lets an interrupt be noticed while stdin is quiet.
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(response) = server.handle_message(&line) {
                    writeln!(stdout_lock, "{response}")?;
                    stdout_lock.flush()?;
                }
            }
            Ok(Err(_)) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}
