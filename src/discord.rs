use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use tokio::sync::mpsc::UnboundedSender;

use crate::{send_or_log, Event};

struct Handler(UnboundedSender<Event>);

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _: Context, msg: Message) {
        send_or_log(&self.0, Event::DiscordMessage(msg));
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        send_or_log(&self.0, Event::DiscordReady(ctx, ready));
    }
}

pub async fn start_discord_integration(
    discord_token: String,
    sender: UnboundedSender<Event>,
) {
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let mut client = Client::builder(discord_token, intents)
        .event_handler(Handler(sender.clone()))
        .await
        .expect("Err creating client");

    println!("Starting discord integration");

    // Finally, start a single shard, and start listening to events.
    //
    // Shards will automatically attempt to reconnect, and will perform
    // exponential backoff until it reconnects.
    if let Err(e) = client.start().await {
        println!("Discord client error: {e}");
    }
}