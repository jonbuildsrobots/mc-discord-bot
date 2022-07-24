use std::collections::HashSet;

use serenity::model::channel::Message;
use serenity::model::gateway::{Ready, Activity};
use serenity::prelude::*;
use serenity::model::id::ChannelId;

use tokio::sync::mpsc;
use tokio::io::AsyncWriteExt;

mod discord;
mod process;
mod stdin_forward;

pub enum Packet {
    DiscordReady(Context, Ready),
    DiscordMessage(Message),
    ProcessStarted(tokio::process::ChildStdin),
    LogLine(String, String),
    StdinLine(String),
    StopServer(),
}

pub fn send_or_log(sender: &mpsc::UnboundedSender<Packet>, packet: Packet) {
    if let Err(_) = sender.send(packet) {
        println!("Error sending internal packet");
    }
}

pub async fn say_or_log(channel_id: ChannelId, ctx: &Context, msg: &str) {
    if let Err(e) = channel_id.say(&ctx.http, msg).await {
        println!("Error sending message: {:?}", e);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        println!("{} TOKEN CHANNEL_ID SERVER_COMMAND SERVER_COMMAND_ARGS...", args[0]);
        return;
    }

    let token = &args[1];
    let channel_id = &args[2];
    let server_command = &args[3];
    let server_command_args = &args[4..];

    println!("Running: {} {:?}", server_command, server_command_args);

    let channel_id: ChannelId = match channel_id.parse() {
        Ok(v) => v,
        Err(_) => {
            println!("Invalid channel id \"{}\"", channel_id);
            return;
        },
    };

    let (sender, receiver) = mpsc::unbounded_channel::<Packet>(); 
    tokio::task::spawn(async move { handle_packets(receiver, channel_id).await });

    let discord_integration = discord::start_discord_integration(token, &sender);
    let process_wrapper = process::start_process_wrapper(server_command, server_command_args, &sender);
    stdin_forward::start_stdin_forwarding(&sender);

    futures::join!(discord_integration, process_wrapper);
}

async fn handle_packets(mut receiver: mpsc::UnboundedReceiver<Packet>, channel_id: ChannelId) {
    let mut ctx: Option<Context> = None;
    let mut stdin: Option<tokio::process::ChildStdin> = None;
    let mut my_id: u64 = 0;
    let mut players_online: HashSet<String> = HashSet::new();

    while let Some(packet) = receiver.recv().await {
        match packet {
            Packet::DiscordReady(new_ctx, ready) => {
                new_ctx.set_activity(Activity::playing(
                    format!("{} Online", players_online.len())
                )).await;

                ctx = Some(new_ctx);
                my_id = ready.user.id.0;
                println!("Discord ready");
            },
            Packet::DiscordMessage(msg) => {
                if msg.author.id == my_id {
                    continue;
                }

                if msg.channel_id != channel_id {
                    continue;
                }

                let ctx = match &ctx {
                    Some(v) => v,
                    None => continue,
                };

                if msg.content == "!help" {
                    say_or_log(channel_id, ctx, "!help - lists commands\n!online - lists online players").await;
                } else if msg.content == "!online" {
                    if players_online.len() == 0 {
                        say_or_log(channel_id, ctx, "No players online").await;
                        continue;
                    }

                    let mut sorted_players: Vec<&String> = players_online.iter().collect();
                    sorted_players.sort();

                    let mut player_list = "Online players: ".to_string();
                    for (i, player) in sorted_players.iter().enumerate() {
                        if i > 0 {
                            player_list.push_str(", ");
                        }
                        player_list.push_str(player);
                    }
                    say_or_log(channel_id, ctx, &player_list).await;
                } else if msg.content.starts_with("!") {
                    say_or_log(channel_id, ctx, &format!("Unknown command: {}", msg.content)).await;
                } else {
                    let stdin = match &mut stdin {
                        Some(v) => v,
                        None => continue,
                    };

                    if let Err(e) = stdin.write(format!("/say {}: {}\r\n", msg.author.name, msg.content_safe(ctx)).as_bytes()).await {
                        println!("Error writing to stdin {}", e);
                    }
                }
            },
            Packet::ProcessStarted(new_stdin) => {
                stdin = Some(new_stdin);
                println!("Process started");
            },
            Packet::LogLine(label, content) => {
                if label == "Server thread/INFO" {
                    if content.starts_with("<") {
                        let end_bracket = content.find("> ");
                        if let Some(end_bracket) = end_bracket {
                            let user = &content[1..end_bracket];
                            let msg = &content[(end_bracket + 2)..];

                            if user == "Server" {
                                continue;
                            }
        
                            let ctx = match &ctx {
                                Some(v) => v,
                                None => continue,
                            };
                            
                            say_or_log(channel_id, ctx, &format!("{}: {}", user, msg)).await;
                        } else {
                            println!("Invalid chat message {}", content);
                        }
                    } else if content.starts_with("Done") {
                        let ctx = match &ctx {
                            Some(v) => v,
                            None => continue,
                        };
                        
                        say_or_log(channel_id, ctx, "Server Started").await;
                    } else if content.ends_with(" joined the game") {
                        let name = &content[0..(content.len() - 16)];
                        players_online.insert(name.to_string());

                        let ctx = match &ctx {
                            Some(v) => v,
                            None => continue,
                        };

                        ctx.set_activity(Activity::playing(
                            format!("{} Online", players_online.len())
                        )).await;

                        say_or_log(channel_id, ctx, &format!("{} joined the server", name)).await;
                    } else if content.ends_with(" left the game") {
                        let name = &content[0..(content.len() - 14)];
                        players_online.remove(name);

                        let ctx = match &ctx {
                            Some(v) => v,
                            None => continue,
                        };

                        ctx.set_activity(Activity::playing(
                            format!("{} Online", players_online.len())
                        )).await;

                        say_or_log(channel_id, ctx, &format!("{} left the server", name)).await;
                    } else {
                        let ctx = match &ctx {
                            Some(v) => v,
                            None => continue,
                        };

                        for player in &players_online {
                            if content.starts_with(player) {
                                say_or_log(channel_id, ctx, &content).await;
                            }
                        }
                    }
                }
            },
            Packet::StdinLine(line) => {
                let stdin = match &mut stdin {
                    Some(v) => v,
                    None => continue,
                };
                
                if let Err(e) = stdin.write(line.as_bytes()).await {
                    println!("Error writing to stdin {}", e);
                }
            },
            Packet::StopServer() => {
                let ctx = match &ctx {
                    Some(v) => v,
                    None => continue,
                };
                
                say_or_log(channel_id, ctx, "Server Shutdown").await;

                std::process::exit(0);
            },
        }
    }
}