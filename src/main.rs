use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::fmt::Write;
use std::io::Write as _;

use log_parser::parse_line;
use process::start_process_wrapper;
use serde::{Serialize, Deserialize};
use serenity::http::Typing;
use serenity::model::channel::Message;
use serenity::model::gateway::{Ready, Activity};
use serenity::prelude::*;
use serenity::model::id::ChannelId;
use tokio::process::{ChildStdin, Command};
use tokio::{fs, task};
use tokio::time::sleep;

use std::fs::OpenOptions;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::io::AsyncWriteExt;

mod discord;
mod log_parser;
mod process;

pub enum Event {
    // Events from the discord integration
    DiscordReady(Context, Ready),
    DiscordMessage(Message),

    // Events from the child process
    StdinLine(String),
    ProcessStopped,

    // Misc events
    InstallComplete,
    CommandTimerElapsed,
}

pub fn send_or_log(sender: &UnboundedSender<Event>, event: Event) {
    if let Err(e) = sender.send(event) {
        println!("Error sending event: {e}");
    }
}

pub async fn say_or_log(channel_id: ChannelId, ctx: &Context, msg: &str) {
    if let Err(e) = channel_id.say(&ctx.http, msg).await {
        println!("Error sending message: {e}");
    }
}

// NOTE(Jon): The only files we need to manually copy over are:
// banned-ips.json, banned-players.json, mc-discord-bot, mc-discord-bot.toml, ops.json, server.properties & whitelist.json

#[derive(Deserialize)]
pub struct ConfigToml {
    // Used for discord integration
    pub discord_token: String,
    pub game_chat_channel: ChannelId,
    pub admin_channel: ChannelId,
    
    // Used for server update (mod/config setup)
    pub modpack_path: String,
    pub client_mods: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct BotStats {
    pub play_times: HashMap<String, u128>,
}

impl BotStats {
    pub fn write(&self) {
        let json_str = serde_json::to_string_pretty(self).unwrap();
        let _ = std::fs::write("mc-discord-bot.json", json_str);
    }
}

const MAX_MESSAGE_LEN: usize = 2000;
const LOG_FRAMING_LEN: usize = 7; // 6 backticks + a newline for a multiline code block
const LOG_BUFFER_LEN: usize = MAX_MESSAGE_LEN - LOG_FRAMING_LEN;    

pub struct BotState {
    pub config: ConfigToml,
    pub stats: BotStats,

    pub sender: UnboundedSender<Event>,
    pub ctx: Option<Context>,
    pub stdin: Option<ChildStdin>,
    pub my_id: u64,
    pub players_online: HashMap<String, Instant>,

    pub log_buffer: String,
    pub task_response_buffer: String,
    pub capture_task_response: bool,
    pub admin_task: Option<Typing>,
}

impl BotState {
    pub async fn start_child_process(&mut self) {
        let stdin = match start_process_wrapper(self.sender.clone()).await {
            Ok(v) => v,
            Err(e) => {
                println!("Error spawning child process: {e}");
                
                if let Some(ctx) = &mut self.ctx {
                    say_or_log(self.config.admin_channel, ctx, &format!("Error spawning child process: {e}")).await;
                }

                return;
            },
        };

        self.stdin = Some(stdin);

        if let Some(ctx) = &mut self.ctx {
            say_or_log(self.config.admin_channel, ctx, "Child process started").await;
            ctx.set_activity(Activity::playing("0 Online")).await;
        }
    }
}

pub fn push_line_to_string_buf(buf: &mut String, line: &str) {
    let line_len = line.len() + 1;
    if line_len > buf.capacity() {
        buf.push_str(&line[..buf.capacity()]);
        return;
    }
    
    let buf_remaining = buf.capacity() - buf.len();
    if line_len > buf_remaining {
        buf.drain(..line_len - buf_remaining);
    }

    buf.push_str(line);
    buf.push('\n');
}

#[tokio::main]
async fn main() {
    // Load the config toml
    let config_toml_string = fs::read_to_string("mc-discord-bot.toml").await.unwrap();
    let config: ConfigToml = toml::from_str(&config_toml_string).unwrap();
    
    // Create the event channel used to send events to the main loop
    let (sender, mut receiver) = unbounded_channel::<Event>();

    // Start the discord integration background task
    task::spawn(discord::start_discord_integration(config.discord_token.clone(), sender.clone())); 
    
    // Load bot stats from json or default initialize
    let stats: BotStats = match fs::read_to_string("mc-discord-bot.json").await {
        Ok(v) => serde_json::from_str(&v).unwrap(),
        Err(_) => BotStats {
            play_times: HashMap::new(),
        },
    };
    
    // Setup bot state
    let mut state = BotState {
        config,
        stats,

        sender,
        ctx: None,
        stdin: None,
        my_id: 0,
        players_online: HashMap::new(),

        log_buffer: String::with_capacity(LOG_BUFFER_LEN),
        task_response_buffer: String::with_capacity(LOG_BUFFER_LEN),
        capture_task_response: false,
        admin_task: None,
    };

    let mut debug_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open("mc-discord-bot-debug.log")
        .expect("Error opening mc-discord-bot-debug.log");

    // Start the child process
    state.start_child_process().await;

    // Start the main event loop
    while let Some(event) = receiver.recv().await {
        match event {
            Event::DiscordReady(new_ctx, ready) => {
                if state.stdin.is_some() {
                    new_ctx.set_activity(Activity::playing(
                        format!("{} Online", state.players_online.len())
                    )).await;
                } else {
                    new_ctx.set_activity(Activity::playing(
                        "Offline".to_string()
                    )).await;
                }

                state.ctx = Some(new_ctx);
                state.my_id = ready.user.id.0;
                println!("Discord ready");
            },
            
            // Skip messages from the bot itself
            Event::DiscordMessage(msg) if msg.author.id == state.my_id => {},
            
            // Handle messages from the game chat channel.
            // These could either be commands starting with a `!` or messages to send to the game chat
            Event::DiscordMessage(msg) if msg.channel_id == state.config.game_chat_channel => {
                let Some(ctx) = &state.ctx else { continue; };

                if msg.content == "!help" {
                    say_or_log(state.config.game_chat_channel, ctx, "**mc-discord-bot Commands**\n`!help` - lists commands\n`!online` - lists online players\n`!time` - lists hours played").await;
                } else if msg.content == "!online" {
                    if state.players_online.len() == 0 {
                        say_or_log(state.config.game_chat_channel, ctx, "No players online").await;
                        continue;
                    }

                    let mut sorted_players: Vec<&String> = state.players_online.keys().collect();
                    sorted_players.sort();

                    let mut player_list = "Online players: ".to_string();
                    for (i, player) in sorted_players.iter().enumerate() {
                        if i > 0 {
                            player_list.push_str(", ");
                        }
                        player_list.push_str(player);
                    }
                    say_or_log(state.config.game_chat_channel, ctx, &player_list).await;
                } else if msg.content == "!time" {
                    let now = Instant::now();
                    
                    // Calculate current play times, taking currently logged in time into account.
                    // Also compute the longest player name
                    let mut curr_play_times = Vec::with_capacity(state.stats.play_times.len());
                    let mut max_player_name = 0;
                    for (player, play_time) in &state.stats.play_times {
                        let curr_play_time = match state.players_online.get(player) {
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
                    say_or_log(state.config.game_chat_channel, ctx, &player_list).await;
                } else if msg.content.starts_with("!") {
                    say_or_log(state.config.game_chat_channel, ctx, &format!("Unknown command: {}", msg.content)).await;
                } else {
                    let Some(stdin) = &mut state.stdin else { continue; };
                    if let Err(e) = stdin.write(format!("/say {}: {}\r\n", msg.author.name, msg.content_safe(ctx)).as_bytes()).await {
                        println!("Error writing to stdin {}", e);
                    }
                }
            },
            
            // Handle messages from the admin channel.
            // These could either be commands starting with a `!` or messages to send directly to stdin
            Event::DiscordMessage(msg) if msg.channel_id == state.config.admin_channel => {
                let Some(ctx) = &state.ctx else { continue; };

                if msg.content == "!help" {
                    say_or_log(state.config.admin_channel, ctx, "**mc-discord-bot Commands**\n`!help` - lists commands\n`!logs` - get server logs\n`!update` - update modpack\n`!start` - start the server if stopped").await;
                } else if msg.content == "!logs" {
                    if state.log_buffer.is_empty() {
                        say_or_log(state.config.admin_channel, ctx, "`No logs`").await;
                    } else {
                        say_or_log(state.config.admin_channel, ctx, &format!("```\n{}```", state.log_buffer)).await;
                    }
                } else if msg.content == "!update" {
                    if state.admin_task.is_some() {
                        say_or_log(state.config.admin_channel, ctx, "Admin task in progress, please wait").await;
                        continue;
                    }

                    if state.stdin.is_some() {
                        say_or_log(state.config.admin_channel, ctx, "Server currently running, run `stop` before updating").await;
                        continue;
                    };

                    state.admin_task = Some(state.config.admin_channel.start_typing(&ctx.http).unwrap());
                    let sender_clone = state.sender.clone();
                    let modpack_path = state.config.modpack_path.clone();
                    // let client_mods = state.config.client_mods.clone();
                    task::spawn(async move {
                        println!("Updating server");

                        // Delete old files if they exist
                        Command::new("rm").args(&["-rf",
                            "config",
                            "defaultconfigs",
                            "kubejs",
                            "libraries",
                            "local",
                            "mods",
                            "patchouli_books",
                            "run.bat",
                            "run.sh",
                        ]).status().await.unwrap();

                        // Generate a few required files
                        fs::write("eula.txt", "eula=true").await.unwrap();
                        fs::write("user_jvm_args.txt", include_str!("user_jvm_args.txt")).await.unwrap();

                        // Download & unzip pack to current directory
                        Command::new("wget").args(&["-O", "pack.zip", &modpack_path]).status().await.unwrap();
                        Command::new("unzip").args(&["pack.zip"]).status().await.unwrap();
                        Command::new("rm").args(&["pack.zip"]).status().await.unwrap();

                        // Run the installer
                        Command::new("java").args(&["-jar", "bin/modpack.jar", "--installServer"]).status().await.unwrap();
                        Command::new("rm").args(&["-rf", "bin", "modpack.jar.log"]).status().await.unwrap();

                        // Delete client mods
                        // for client_mod in &client_mods {
                        //     println!("Removing client mod {client_mod}");
                        //     let _ = Command::new("rm").args(&[format!("mods/{client_mod}")]).status();
                        // }

                        println!("Update complete");
                        let _ = sender_clone.send(Event::InstallComplete);
                    });
                } else if msg.content == "!start" {
                    if state.stdin.is_some() {
                        say_or_log(state.config.admin_channel, ctx, "`Child process already running`").await;
                    } else {
                        state.start_child_process().await;
                    }
                } else if msg.content.starts_with("!") {
                    say_or_log(state.config.admin_channel, ctx, &format!("Unknown command: {}", msg.content)).await;
                } else {
                    if state.admin_task.is_some() {
                        say_or_log(state.config.admin_channel, ctx, "Admin task in progress, please wait").await;
                        continue;
                    }

                    let Some(stdin) = &mut state.stdin else {
                        say_or_log(state.config.admin_channel, ctx, "No process currently running").await;
                        continue;
                    };

                    // Send to stdin
                    println!("{}", msg.content);
                    match stdin.write(format!("{}\r\n", msg.content).as_bytes()).await {
                        Ok(_) => {
                            state.admin_task = Some(state.config.admin_channel.start_typing(&ctx.http).unwrap());
                            state.task_response_buffer.clear();
                            state.capture_task_response = true;

                            let sender_clone = state.sender.clone();
                            task::spawn(async move {
                                sleep(Duration::from_millis(1000)).await;
                                let _ = sender_clone.send(Event::CommandTimerElapsed);
                            });
                        },
                        Err(e) => {
                            println!("Error writing to stdin {}", e);
                        },
                    }
                }
            },

            // Handle the child process stopping (eg. due to a stop command or a server crash)
            Event::ProcessStopped => {
                state.stdin = None;
                
                let Some(ctx) = &state.ctx else { continue; };
                say_or_log(state.config.game_chat_channel, ctx, "Server Shutdown").await;
                ctx.set_activity(Activity::playing(
                    "Offline".to_string()
                )).await;
            },

            // Sent when the pack has been installed & we are ready to start the child process
            Event::InstallComplete => {
                state.admin_task = None;
                state.start_child_process().await;
            },

            // Sent when a command's log timer has elapsed
            Event::CommandTimerElapsed => {
                println!("Command timer elapsed");
                state.admin_task = None;

                let Some(ctx) = &state.ctx else { continue; };
                if state.task_response_buffer.is_empty() {
                    say_or_log(state.config.admin_channel, ctx, "`No response`").await;
                } else {
                    say_or_log(state.config.admin_channel, ctx, &format!("```\n{}```", state.task_response_buffer)).await;
                }
            },

            // Handle log lines from the child process (the minecraft server)
            Event::StdinLine(line) => {
                let Some(ctx) = &state.ctx else { continue; };

                // Add the line to the log buffer (`line` will only contain ascii characters since the stdin code
                // in process.rs removes non-ascii characters before sending this event)
                push_line_to_string_buf(&mut state.log_buffer, &line);

                if state.capture_task_response {
                    push_line_to_string_buf(&mut state.task_response_buffer, &line);
                }

                // Parse & handle the log line
                let Ok((label, content)) = parse_line(&line) else { continue; };
                match label {
                    // Server startup
                    "minecraft/DedicatedServer" if content.starts_with("Done") => {
                        say_or_log(state.config.game_chat_channel, ctx, "Server Started").await;
                    },

                    // Player login
                    "minecraft/MinecraftServer" if content.ends_with(" joined the game") => {
                        let name = &content[0..(content.len() - 16)];
                        let now = Instant::now();
                        state.players_online.insert(name.to_string(), now);
                        let _ = writeln!(&mut debug_log, "{name} Joined: {now:?}");

                        if !state.stats.play_times.contains_key(name) {
                            state.stats.play_times.insert(name.to_string(), 0);
                        }
                        
                        ctx.set_activity(Activity::playing(
                            format!("{} Online", state.players_online.len())
                        )).await;

                        say_or_log(state.config.game_chat_channel, ctx, &format!("{} joined the server", name)).await;
                    },

                    // Player logout
                    "minecraft/MinecraftServer" if content.ends_with(" left the game") => {
                        let name = &content[0..(content.len() - 14)];
                        if let Some(login_time) = state.players_online.remove(name) {
                            // Update play time
                            let mut play_time = state.stats.play_times.get(name).cloned().unwrap_or(0);
                            let now = Instant::now();
                            let dt = now - login_time;
                            play_time += dt.as_millis();
                            let _ = writeln!(&mut debug_log, "{name} Left: login time {login_time:?}, logout time {now:?}, dt millis {}, play time {play_time}", dt.as_millis());

                            state.stats.play_times.insert(name.to_string(), play_time);
                            state.stats.write();
                        }

                        ctx.set_activity(Activity::playing(
                            format!("{} Online", state.players_online.len())
                        )).await;

                        say_or_log(state.config.game_chat_channel, ctx, &format!("{} left the server", name)).await;
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

                            say_or_log(state.config.game_chat_channel, ctx, &format!("{}: {}", user, msg)).await;
                        } else {
                            println!("Invalid chat message {}", content);
                        }
                    },

                    // Handle misc other messages (eg. PLAYER fell out of the world)
                    "minecraft/MinecraftServer" => {
                        for player in state.players_online.keys() {
                            if content.starts_with(player) {
                                say_or_log(state.config.game_chat_channel, ctx, &content).await;
                            }
                        }
                    },

                    _ => {},
                }
            },

            _ => {},
        }
    }
}