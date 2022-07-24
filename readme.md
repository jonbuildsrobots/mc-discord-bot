## Minecraft Discord Bot

To build for Windows run `cargo build --release --target=x86_64-pc-windows-msvc`, the produced executable will be `target/x86_64-pc-windows-msvc/release/mc-discord-bot.exe`

To build for Linux run `cargo build --release --target=x86_64-unknown-linux-gnu`, the produced executable will be `target/x86_64-unknown-linux-gnu/release/mc-discord-bot`

To use the discord bot run `mc-discord-bot.exe TOKEN CHANNEL_ID SERVER_COMMAND SERVER_COMMAND_ARGS...`, for example `mc-discord-bot.exe "mydiscordtokenhere" 123456789123456789 java -jar server.jar nogui`

### Getting a discord bot token
1. Go to https://discord.com/developers/applications
2. Press `New Application`, enter a reasonably unique name, then press `Create`
3. Go to the `Bot` tab
4. Press `Add Bot` then `Yes, do it!`
5. Press `Reset Token`, then press `Yes, do it!`, then press `Copy` to copy your discord bot token to your clipboard. Save this somewhere private
6. Go to the `OAuth2` tab, then go to the `URL Generator` subtab
7. Check the `bot` checkbox in the section labeled `SCOPES` 
8. Check the `Send Messages` checkbox in the section labeled `BOT PERMISSIONS`
9. Open the url at the bottom of the page, labeled `GENERATED URL`, in a different tab. This is how you add the bot to a server
10. Use the token from step 5 as the first argument to the bot

### Getting a channel ID
1. Go to settings in the discord app on desktop
2. Go to `Advanced`
3. Make sure `Developer Mode` is turned on
4. Right click on the channel you want the ID of and press `Copy ID`
5. Use the ID from step 4 as the second argument to the bot
