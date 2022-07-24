use tokio::sync::mpsc;
use crate::{Packet, send_or_log};

use std::io::BufRead;

pub fn start_stdin_forwarding(sender: &mpsc::UnboundedSender<Packet>) {
    let sender = sender.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();

        loop {
            let mut buffer = String::new();
            if let Err(e) = handle.read_line(&mut buffer) {
                println!("Error reading line {}", e);
                continue;    
            }

            send_or_log(&sender, Packet::StdinLine(buffer));
        }
    });
}