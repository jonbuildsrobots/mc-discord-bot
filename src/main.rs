use std::collections::HashMap;
use std::process::Command;
use std::{fs, env};
use std::time::Instant;
use std::fmt::Write;
use std::io::Write as _;

use serde::{Serialize, Deserialize};
use serenity::model::channel::Message;
use serenity::model::gateway::{Ready, Activity};
use serenity::prelude::*;
use serenity::model::id::ChannelId;

use std::fs::OpenOptions;
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

#[derive(Deserialize)]
pub struct ConfigToml {
    // Used for discord integration
    pub discord_token: String,
    pub discord_channel_id: String,
    
    // Used for server setup
    pub server_setup_url: String,
    
    // Used for server update (mod/config setup)
    pub modpack_path: String,
    pub client_mods: Vec<String>,
}

#[tokio::main]
async fn main() {
    let config_toml_string = fs::read_to_string("mc-discord-bot.toml").unwrap();
    let config_toml: ConfigToml = toml::from_str(&config_toml_string).unwrap();
    
    let channel_id: ChannelId = match config_toml.discord_channel_id.parse() {
        Ok(v) => v,
        Err(_) => {
            println!("Invalid channel id \"{}\"", config_toml.discord_channel_id);
            return;
        },
    };

    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        if args[1] == "setup" {
            // NOTE(Jon): The only files we need to manually copy over are:
            // banned-ips.json, banned-players.json, mc-discord-bot, mc-discord-bot.toml, ops.json, server.properties & whitelist.json

            println!("Setting up server");
            let _ = Command::new("wget").args(&["-O", "installer.jar", &config_toml.server_setup_url]).status();
            let _ = Command::new("java").args(&["-jar", "installer.jar", "--installServer"]).status();
            let _ = Command::new("rm").args(&["installer.jar", "installer.jar.log"]).status();
            let _ = fs::write("eula.txt", "eula=true");
            let _ = fs::write("user_jvm_args.txt", include_str!("user_jvm_args.txt"));
            return;
        } else if args[1] == "update" {
            println!("Updating server");
            let _ = Command::new("wget").args(&["-O", "pack.zip", &config_toml.modpack_path]).status();
            let _ = Command::new("unzip").args(&["pack.zip", "-d", "temp-pack"]).status();
            let _ = Command::new("rm").args(&["pack.zip"]).status();
            let _ = Command::new("rm").args(&["-rf", "mods", "config", "defaultconfigs"]).status();
            let _ = Command::new("cp").args(&["-r", "temp-pack/.minecraft/mods", "temp-pack/.minecraft/config", "temp-pack/.minecraft/defaultconfigs", "."]).status();
            let _ = Command::new("rm").args(&["-rf", "temp-pack"]).status();

            for client_mod in &config_toml.client_mods {
                println!("Removing client mod {client_mod}");
                let _ = Command::new("rm").args(&[format!("mods/{client_mod}")]).status();
            }

            return;
        } else {
            println!("Invalid command \"{}\"", args[1]);
            return;
        }
    }

    let (sender, receiver) = mpsc::unbounded_channel::<Packet>(); 
    tokio::task::spawn(async move { handle_packets(receiver, channel_id).await });

    let discord_integration = discord::start_discord_integration(&config_toml.discord_token, &sender);
    let process_wrapper = process::start_process_wrapper("./run.sh", &[], &sender);
    stdin_forward::start_stdin_forwarding(&sender);

    // TODO(Jon): Remove this so we can remove "futures" as a dependency 
    futures::join!(discord_integration, process_wrapper);
}

#[derive(Serialize, Deserialize)]
pub struct BotState {
    pub play_times: HashMap<String, u128>,
}

impl BotState {
    pub fn write(&self) {
        let json_str = serde_json::to_string_pretty(self).unwrap();
        let _ = std::fs::write("mc-discord-bot.json", json_str);
    }
}

async fn handle_packets(mut receiver: mpsc::UnboundedReceiver<Packet>, channel_id: ChannelId) {
    let mut ctx: Option<Context> = None;
    let mut stdin: Option<tokio::process::ChildStdin> = None;
    let mut my_id: u64 = 0;
    let mut players_online: HashMap<String, Instant> = HashMap::new();
    
    let mut state: BotState = match fs::read_to_string("mc-discord-bot.json") {
        Ok(v) => serde_json::from_str(&v).unwrap(),
        Err(_) => BotState{
            play_times: HashMap::new(),
        },
    };

    let mut debug_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open("mc-discord-bot-debug.log")
        .expect("Error opening mc-discord-bot-debug.log");

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
                    say_or_log(channel_id, ctx, "**mc-discord-bot Commands**\n`!help` - lists commands\n`!online` - lists online players\n`!time` - lists hours played").await;
                } else if msg.content == "!online" {
                    if players_online.len() == 0 {
                        say_or_log(channel_id, ctx, "No players online").await;
                        continue;
                    }

                    let mut sorted_players: Vec<&String> = players_online.keys().collect();
                    sorted_players.sort();

                    let mut player_list = "Online players: ".to_string();
                    for (i, player) in sorted_players.iter().enumerate() {
                        if i > 0 {
                            player_list.push_str(", ");
                        }
                        player_list.push_str(player);
                    }
                    say_or_log(channel_id, ctx, &player_list).await;
                } else if msg.content == "!time" {
                    let now = Instant::now();
                    
                    // Calculate current play times, taking currently logged in time into account.
                    // Also compute the longest player name
                    let mut curr_play_times = Vec::with_capacity(state.play_times.len());
                    let mut max_player_name = 0;
                    for (player, play_time) in &state.play_times {
                        let curr_play_time = match players_online.get(player) {
                            Some(login_time) => *play_time + (now - *login_time).as_millis(),
                            None => *play_time,
                        };
                        
                        max_player_name = max_player_name.max(player.len()); 
                        curr_play_times.push((player, curr_play_time));
                    }

                    curr_play_times.sort_by_key(|x| x.1);
                    
                    let mut player_list = "```Total play time:\n".to_string();
                    for (player, play_time) in curr_play_times.iter().rev() {
                        let total_hours = (*play_time as f64) / 3600000.0;
                        // let days = play_time / ;
                        let _ = writeln!(&mut player_list, "{player: <max_player_name$} | {total_hours: <6.2} hr");
                    }
                    let _ = write!(&mut player_list, "```");
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
                let ctx = match &ctx {
                    Some(v) => v,
                    None => continue,
                };

                match label.as_str() {
                    // Server startup
                    "minecraft/DedicatedServer" if content.starts_with("Done") => {
                        say_or_log(channel_id, ctx, "Server Started").await;
                    },

                    // Player login
                    "minecraft/MinecraftServer" if content.ends_with(" joined the game") => {
                        let name = &content[0..(content.len() - 16)];
                        let now = Instant::now();
                        players_online.insert(name.to_string(), now);
                        let _ = writeln!(&mut debug_log, "{name} Joined: {now:?}");

                        if !state.play_times.contains_key(name) {
                            state.play_times.insert(name.to_string(), 0);
                        }
                        
                        ctx.set_activity(Activity::playing(
                            format!("{} Online", players_online.len())
                        )).await;

                        say_or_log(channel_id, ctx, &format!("{} joined the server", name)).await;
                    },

                    // Player logout
                    "minecraft/MinecraftServer" if content.ends_with(" left the game") => {
                        let name = &content[0..(content.len() - 14)];
                        if let Some(login_time) = players_online.remove(name) {
                            // Update play time
                            let mut play_time = state.play_times.get(name).cloned().unwrap_or(0);
                            let now = Instant::now();
                            let dt = now - login_time;
                            play_time += dt.as_millis();
                            let _ = writeln!(&mut debug_log, "{name} Left: login time {login_time:?}, logout time {now:?}, dt millis {}, play time {play_time}", dt.as_millis());

                            state.play_times.insert(name.to_string(), play_time);
                            state.write();
                        }

                        ctx.set_activity(Activity::playing(
                            format!("{} Online", players_online.len())
                        )).await;

                        say_or_log(channel_id, ctx, &format!("{} left the server", name)).await;
                    },

                    // Chat message
                    "minecraft/MinecraftServer" if content.starts_with("<") => {
                        let end_bracket = content.find("> ");
                        if let Some(end_bracket) = end_bracket {
                            let user = &content[1..end_bracket];
                            let msg = &content[(end_bracket + 2)..];

                            if user == "Server" {
                                continue;
                            }

                            say_or_log(channel_id, ctx, &format!("{}: {}", user, msg)).await;
                        } else {
                            println!("Invalid chat message {}", content);
                        }
                    },

                    // Handle misc other messages (eg. PLAYER fell out of the world)
                    "minecraft/MinecraftServer" => {
                        for player in players_online.keys() {
                            if content.starts_with(player) {
                                say_or_log(channel_id, ctx, &content).await;
                            }
                        }
                    },

                    _ => {},
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