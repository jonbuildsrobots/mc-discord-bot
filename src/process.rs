use tokio::task;
use tokio::{process::ChildStdin, sync::mpsc::UnboundedSender};
use tokio::process::Command;
use std::process::Stdio;
use tokio::io::{self, AsyncReadExt};

use crate::{Event, send_or_log};

fn spawn_line_processing_task<T: AsyncReadExt + Unpin + Send + 'static>(mut stdio: T, sender: UnboundedSender<Event>) {
    task::spawn(async move {
        let mut used: usize = 0;
        let mut buffer: [u8; 1000] = [0; 1000];
        loop {
            // drop data if buffer fills without any lines
            if used == buffer.len() {
                println!("Buffer filled, dropping data");
                used = 0;
            }

            // read from 
            let bytes_read = match stdio.read(&mut buffer[used..]).await {
                Ok(v) => v,
                Err(_) => break,
            };

            let old_used = used;
            used += bytes_read;

            // process completed lines
            let mut line_start: usize = 0;
            for i in old_used..used {
                if buffer[i] == ('\n' as u8) {
                    let line_end = if (line_start < i) && (buffer[i - 1] == '\r' as u8) { i - 1 } else { i };

                    let line = match std::str::from_utf8(&buffer[line_start..line_end]) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("Error: {}", e);
                            continue;
                        },
                    };
                    
                    // Print line & advance line_start
                    println!("{line}");
                    line_start = i + 1;
                    
                    // Remove non-ascii characters & send cleaned line as an avent
                    let cleaned_line: String = line
                        .chars()
                        .filter(|c| c.is_ascii())
                        .collect();
                    send_or_log(&sender, Event::StdinLine(cleaned_line));
                }
            }

            // shift buffer downwards to remove processed data
            used -= line_start;
            for i in 0..used {
                buffer[i] = buffer[i + line_start];
            }
        }

        println!("stdio loop exited");
    });
}

pub async fn start_process_wrapper(
    sender: UnboundedSender<Event>,
) -> Result<ChildStdin, io::Error> {
    println!("Spawning child process");
    let mut child = Command::new("./run.sh")
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    let stdin = child.stdin.take().expect("child did not have a handle to stdin");
    
    let stdout = child.stdout.take().expect("child did not have a handle to stdout");
    spawn_line_processing_task(stdout, sender.clone());
    
    let stderr = child.stderr.take().expect("child did not have a handle to stderr");
    spawn_line_processing_task(stderr, sender.clone());

    task::spawn(async move {
        let exit_status = child.wait().await;
        println!("process exited {:?}", exit_status);
        send_or_log(&sender, Event::ProcessStopped);
    });

    Ok(stdin)
}