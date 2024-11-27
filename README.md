# aghast

When your users are left aghast... let them report it!

The bot is configured entirely with the environment:

```dotenv
AGHAST_CHANNEL=<forum channel to post in>
AGHAST_GUILD=<guild for which the bot will be used>
AGHAST_DB_URL=<"sqlite://aghast.db" or a similar sqlite url>
AGHAST_TOKEN=<discord bot token>
AGHAST_OPEN_MESSAGE=<message to send to a user to confirm a report has been opened>
AGHAST_CLOSE_MESSAGE=<message to send a user when a report is closed>
```
