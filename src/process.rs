use tokio::sync::mpsc;
use tokio::process::Command;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

use crate::{Packet, send_or_log};

pub fn parse_line(line: &str) -> Result<(&str, &str), &'static str> {
    if line.len() <= 11 {
        return Err("too short");
    }
    
    // ensure line is formatted as such
    // [__:__:__] [
    let line_bytes = line.as_bytes();
    if (line_bytes[0]  != '[' as u8) ||
       (line_bytes[3]  != ':' as u8) ||
       (line_bytes[6]  != ':' as u8) ||
       (line_bytes[9]  != ']' as u8) ||
       (line_bytes[10] != ' ' as u8) ||
       (line_bytes[11] != '[' as u8)
    {
        return Err("invalid format");
    }

    let mut label_end = 12;
    while label_end < line_bytes.len() {
        if line_bytes[label_end] == (']' as u8) { break; }

        label_end += 1;
    }

    if label_end == line_bytes.len() {
        return Err("no label found");
    }

    // Assuming lines are formatted as follows 
    // [__:__:__] [????]: ???
    // This means that content starts 3 bytes after the ']' label end
    let content_start = label_end + 3;

    // ensure content_start is within the line's length 
    if line_bytes.len() <= content_start {
        return Err("invalid content");
    }

    let label = match std::str::from_utf8(&line_bytes[12..label_end]) {
        Ok(v) => v,
        Err(_) => return Err("label not utf8"),
    };

    let content = match std::str::from_utf8(&line_bytes[content_start..]) {
        Ok(v) => v,
        Err(_) => return Err("content not utf8"),
    };

    return Ok((label, content));
}

fn process_line(line: &str, sender: &mpsc::UnboundedSender<Packet>) {
    let (label, content) = match parse_line(line) {
        Ok(v) => v,
        Err(e) => {
            println!("{} {}", e, line);
            return;
        },
    };

    send_or_log(sender, Packet::LogLine(label.to_string(), content.to_string()));
    println!("Processed [{}] {}", label, content);
}

fn spawn_line_processing_task<T: AsyncReadExt + Unpin + Send + 'static>(mut stdio: T, sender: mpsc::UnboundedSender<Packet>) {
    tokio::task::spawn(async move {
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

                    process_line(line, &sender);
                    line_start = i + 1;
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

pub async fn start_process_wrapper(server_command: &str, server_command_args: &[String], sender: &mpsc::UnboundedSender<Packet>) {
    let mut cmd = Command::new(server_command);
    cmd.args(server_command_args);

    cmd.stdout(Stdio::piped());
    cmd.stdin(Stdio::piped());
    cmd.stderr(Stdio::piped());

    println!("Spawning child process");
    let mut child = cmd.spawn().expect("failed to spawn command");
    
    let stdin = child.stdin.take().expect("child did not have a handle to stdin");
    send_or_log(sender, Packet::ProcessStarted(stdin));
    
    let stdout = child.stdout.take().expect("child did not have a handle to stdout");
    spawn_line_processing_task(stdout, sender.clone());
    
    let stderr = child.stderr.take().expect("child did not have a handle to stderr");
    spawn_line_processing_task(stderr, sender.clone());

    let exit_status = child.wait().await;
    println!("process exited {:?}", exit_status);

    send_or_log(sender, Packet::StopServer());
}

#[cfg(test)]
mod tests {
    use crate::process::parse_line;

    #[test]
    fn test_parse_line() {
        assert_eq!(parse_line("[__:__:__] [TEST1]: content").unwrap(), ("TEST1", "content"));
        assert_eq!(parse_line("[__:__:__] [TEST2]: A").unwrap(), ("TEST2", "A"));
        assert_eq!(parse_line("[__:__:__] [TEST3]: ").unwrap_err(), "invalid content");
        assert_eq!(parse_line("[__:__:__] [").unwrap_err(), "no label found");
        assert_eq!(parse_line("[__:__:__] [abcdefg").unwrap_err(), "no label found");
        assert_eq!(parse_line("[__:__:__] ").unwrap_err(), "too short");
        assert_eq!(parse_line("A__:__:__] [").unwrap_err(), "invalid format");
    }
}