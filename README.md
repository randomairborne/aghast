# aghast

When your users are left aghast... let them report it!

A modmail bot taking DMs and putting them into a forum, for your moderators to reply to.

`!r <message>` in a ticket thread to reply, `!close` to close the chat. Just DM the bot to open a new ticket.

Run the bot with docker. There is a compose.yaml file in the root of this repository. It will load a
aghast.env file and create an `aghast-data/` folder.

The bot is configured entirely with the environment:

```dotenv
AGHAST_CHANNEL=<forum channel to post in>
AGHAST_GUILD=<guild for which the bot will be used>
AGHAST_TOKEN=<discord bot token>
AGHAST_OPEN_MESSAGE=<message to send to a user to confirm a report has been opened>
AGHAST_CLOSE_MESSAGE=<message to send a user when a report is closed>
```

If you're running the bot outside of docker, you can also set `AGHAST_DB_URL` seperately.

```dotenv
AGHAST_DB_URL=<"sqlite://aghast.db" or a similar sqlite url>
```
