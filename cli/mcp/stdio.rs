use super::protocol::JsonRpcRequest;
use super::TursoMcpServer;
use anyhow::Result;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

impl TursoMcpServer {
    pub fn run(&self) -> Result<()> {
        let stdout = io::stdout();
        let mut stdout_lock = stdout.lock();

        // Create a channel to receive lines from stdin
        let (tx, rx) = mpsc::channel();

        // Spawn a thread to read from stdin
        thread::spawn(move || {
            let stdin = io::stdin();
            let reader = BufReader::new(stdin);

            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if tx.send(Ok(line)).is_err() {
                            break; // Main thread has dropped the receiver
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });

        loop {
            // Check if we've been interrupted
            if self.interrupt_count.load(Ordering::SeqCst) > 0 {
                eprintln!("MCP server interrupted, shutting down...");
                break;
            }

            // Try to receive a line with a timeout so we can check for interruption
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }

                    let request: JsonRpcRequest = match serde_json::from_str(&line) {
                        Ok(req) => req,
                        Err(e) => {
                            eprintln!("Failed to parse JSON-RPC request: {e}");
                            continue;
                        }
                    };

                    let response = self.handle_request(request);
                    // Don't send a response for notifications (when id is None)
                    if response.id.is_some() || response.error.is_some() {
                        let response_json = serde_json::to_string(&response)?;
                        writeln!(stdout_lock, "{response_json}")?;
                        stdout_lock.flush()?;
                    }
                }
                Ok(Err(_)) => {
                    // Error reading from stdin
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Timeout - continue loop to check for interruption
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Stdin thread has finished (EOF)
                    break;
                }
            }
        }

        Ok(())
    }
}
