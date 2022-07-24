use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use tokio::sync::mpsc;

use crate::{Packet, send_or_log};

struct Handler(mpsc::UnboundedSender<Packet>);

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _: Context, msg: Message) {
        send_or_log(&self.0, Packet::DiscordMessage(msg));
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        send_or_log(&self.0, Packet::DiscordReady(ctx, ready));
    }
}

pub async fn start_discord_integration(token: &str, sender: &mpsc::UnboundedSender<Packet>) {
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let mut client = Client::builder(token, intents)
        .event_handler(Handler(sender.clone()))
        .await
        .expect("Err creating client");

    println!("Starting discord integration");

    // Finally, start a single shard, and start listening to events.
    //
    // Shards will automatically attempt to reconnect, and will perform
    // exponential backoff until it reconnects.
    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }

    send_or_log(sender, Packet::StopServer());
}