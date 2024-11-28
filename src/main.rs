#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
use std::{borrow::Cow, fmt::Debug, sync::Arc};

use sqlx::{
    query,
    sqlite::{SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use twilight_gateway::{CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::{request::channel::reaction::RequestReactionType, Client};
use twilight_model::{
    channel::{
        message::{
            component::{ActionRow, Button, ButtonStyle},
            AllowedMentions, Component, ReactionType,
        },
        Attachment, Message,
    },
    gateway::{
        payload::{incoming::MessageUpdate, outgoing::UpdatePresence},
        presence::{Activity, ActivityType, Status},
    },
    http::attachment::Attachment as HttpAttachment,
    id::{
        marker::{ChannelMarker, GuildMarker, MessageMarker, UserMarker},
        Id,
    },
    user::User,
};
use valk_utils::{get_var, parse_var};

const HELP_MESSAGE: &str =
    "`!r <message>` to reply to a ticket, `!close` to close a ticket, `!help` to show this message.";

fn main() {
    let token = get_var("AGHAST_TOKEN");
    let database: SqliteConnectOptions = parse_var("AGHAST_DB_URL");
    let channel: Id<ChannelMarker> = parse_var("AGHAST_CHANNEL");
    let guild: Id<GuildMarker> = parse_var("AGHAST_GUILD");
    let open_message: Arc<str> = get_var("AGHAST_OPEN_MESSAGE").into();
    let close_message: Arc<str> = get_var("AGHAST_CLOSE_MESSAGE").into();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("aghast-main")
        .build()
        .unwrap();

    let intents = Intents::DIRECT_MESSAGES
        | Intents::GUILD_MESSAGES
        | Intents::GUILDS
        | Intents::MESSAGE_CONTENT;
    let client = Client::new(token.clone());
    let shard = rt.block_on(async { twilight_gateway::Shard::new(ShardId::ONE, token, intents) });
    let shard_handle = shard.sender();

    let presence =
        UpdatePresence::new([get_activity()], false, None, Status::Online).expect("Bad presence");
    shard.command(&presence);

    let tasks = TaskTracker::new();
    let cancel = CancellationToken::new();

    let db_opts = database
        .create_if_missing(true)
        .optimize_on_close(true, None)
        .auto_vacuum(SqliteAutoVacuum::Incremental)
        .journal_mode(SqliteJournalMode::Wal);
    let db = rt
        .block_on(SqlitePoolOptions::new().connect_with(db_opts))
        .expect("Failed to connect to db");

    rt.block_on(sqlx::migrate!().run(&db))
        .expect("Failed to run migrations");


    let bot_id = rt.block_on(async {
        client.current_user().await.expect("Failed to get current user").model().await.expect("Failed to deserialize current user").id
    });

    let state = AppState {
        client: Arc::new(client),
        channel,
        guild,
        bot_id,
        db: db.clone(),
        open_message,
        close_message,
    };

    let main = rt.spawn(event_loop(state, shard, cancel.clone(), tasks.clone()));

    eprintln!("Event loop started");

    rt.block_on(async move {
        // we need to do all these things in this order so that we tell everything to shut down,
        // then wait for it to shut down, then clean up
        vss::shutdown_signal().await;
        eprintln!("Shutting down");
        cancel.cancel();
        let _ = shard_handle
            .close(CloseFrame::NORMAL)
            .inspect_err(report_inspect);
        let _ = main.await.inspect_err(report_inspect);
        tasks.close();
        tasks.wait().await;
        db.close().await;
    });
}

fn get_activity() -> Activity {
    Activity {
        application_id: None,
        assets: None,
        buttons: Vec::new(),
        created_at: None,
        details: None,
        emoji: None,
        flags: None,
        id: None,
        instance: None,
        kind: ActivityType::Watching,
        name: "DM to talk to moderators!".to_string(),
        party: None,
        secrets: None,
        state: None,
        timestamps: None,
        url: None,
    }
}

async fn event_loop(
    state: AppState,
    mut shard: Shard,
    cancel: CancellationToken,
    tasks: TaskTracker,
) {
    let events = EventTypeFlags::MESSAGE_CREATE
        | EventTypeFlags::MESSAGE_UPDATE
        | EventTypeFlags::THREAD_DELETE
        | EventTypeFlags::THREAD_UPDATE;
    while let Some(event) = shard.next_event(events).await {
        let event = match event {
            // we special-case gateway close so that we properly exit from the shard handle close
            Ok(Event::GatewayClose(_)) if cancel.is_cancelled() => {
                break;
            }
            Ok(v) => v,
            // an error occurs if we try to send a command while the gateway is starting.
            // if we're shutting down anyway... just break
            Err(e) => {
                eprintln!("ERROR: {e:?}");
                if cancel.is_cancelled() {
                    break;
                }
                continue;
            }
        };
        tasks.spawn(handle_event_ew(state.clone(), event));
    }
}

async fn handle_event_ew(state: AppState, event: Event) {
    if let Err(e) = handle_event(state, event).await {
        eprintln!("ERROR: {e:?}");
    }
}

async fn handle_event(state: AppState, event: Event) -> Result<(), Error> {
    match event {
        Event::MessageCreate(mc) => Box::pin(handle_message(state, mc.0)).await?,
        Event::MessageUpdate(mu) => Box::pin(handle_message_update(state, *mu)).await?,
        Event::ThreadDelete(td) if td.parent_id == state.channel => {
            handle_thread_delete(state, td.id).await;
        }
        Event::ThreadUpdate(tu)
            if tu.parent_id == Some(state.channel)
                && tu
                    .thread_metadata
                    .as_ref()
                    .is_some_and(|v| v.archived || v.locked) =>
        {
            handle_thread_delete(state, tu.id).await;
        }
        _ => {}
    };
    Ok(())
}

async fn handle_message(state: AppState, mc: Message) -> Result<(), Error> {
    if mc.author.bot {
        // do nothing
    } else if mc.guild_id.is_some_and(|id| id == state.guild) {
        if let Some(ticket_user) = get_ticket(&state.db, mc.channel_id).await? {
            handle_guild_message_error_report_wrapper(state, mc, ticket_user).await?;
        }
    } else if mc.guild_id.is_none() {
        handle_user_message(state, mc).await?;
    }
    Ok(())
}

async fn handle_message_update(state: AppState, mu: MessageUpdate) -> Result<(), Error> {
    // we can't do things with no content or author
    let (Some(content), Some(author)) = (mu.content, mu.author) else {
        return Ok(());
    };

    // if we edited the message, someone else created the counterpart- and we aren't allowed to edit it.
    if author.id == state.bot_id {
        return Ok(())
    }

    // if a user already got a close message, they won't be suprised their edits aren't tracked.
    let counterparted = get_counterpart(&state.db, mu.id)
        .await?
        .ok_or(Error::NoCounterpart)?;


    // this means the edited message was in DMs, so the first branch of the if is for editing in
    // the forum.
    let (to_edit, content) = if counterparted.dm.channel == mu.channel_id {
        (
            counterparted.thread,
            Cow::Owned(format!("<@{}>: {content}", author.id)),
        )
    } else if !content.starts_with("!r ") { // We don't want to mess with things that don't start with !r outside of dms
        return Ok(());
    } else {
        (
            counterparted.dm,
            Cow::Borrowed(content.trim_start_matches("!r").trim()),
        )
    };

    eprintln!("editing message {}", to_edit.message);

    state
        .client
        .update_message(to_edit.channel, to_edit.message)
        .content(Some(&content))
        .await?;
    Ok(())
}

async fn handle_guild_message_error_report_wrapper(
    state: AppState,
    mc: Message,
    ticket_channels: TicketChannels,
) -> Result<(), Error> {
    let channel_id = mc.channel_id;
    let reply_id = mc.id;
    if let Err(e) = handle_guild_message(state.clone(), mc, ticket_channels).await {
        eprintln!("{e:?}");
        let msg = format!("Error handling command: `{e}`");
        state
            .client
            .create_message(channel_id)
            .content(&msg)
            .reply(reply_id)
            .await?;
    }
    Ok(())
}

async fn handle_guild_message(
    state: AppState,
    mc: Message,
    ticket_channels: TicketChannels,
) -> Result<(), Error> {
    if mc.content == "!help" {
        state
            .client
            .create_message(mc.channel_id)
            .reply(mc.id)
            .content(HELP_MESSAGE)
            .await?;
    } else if mc.content == "!close" {
        delete_ticket(&state.db, ticket_channels.thread).await?;
        let msg = if let Err(e) = state
            .client
            .create_message(ticket_channels.dm)
            .content(&state.close_message)
            .await
        {
            format!("Could not inform user of thread closing: ```{e:?}```")
        } else {
            "Closed thread!".to_string()
        };
        state
            .client
            .create_message(mc.channel_id)
            .reply(mc.id)
            .content(&msg)
            .await?;
    } else if mc.content.starts_with("!r") {
        let content = &mc.content.trim_start_matches("!r").trim();
        let (attachments, errors) = attachments_to_attachments(&mc.attachments).await;
        let dm_message = state
            .client
            .create_message(ticket_channels.dm)
            .content(content)
            .attachments(&attachments)
            .await?.model().await?;
        if !errors.is_empty() {
            // each error is independently fallible, so we send as much as we can
            let error_report: String =
                std::iter::once("# errors encountered uploading attachments:\n".to_string())
                    .chain(errors.iter().map(|e| format!("`{e:?}`\n")))
                    .collect();
            state
                .client
                .create_message(mc.channel_id)
                .content(&error_report)
                .await?;
        }

        store_message_counterpart(&state.db, ticket_channels, mc.id, dm_message.id).await?;
        message_ack(&state.client, mc.channel_id, mc.id).await?;
    }
    Ok(())
}

async fn handle_user_message(state: AppState, mc: Message) -> Result<(), Error> {
    let components = attachments_to_components(mc.attachments.clone());
    let message = format!("<@{}>: {}", mc.author.id, mc.content);

    let Some(channels) = get_ticket(&state.db, mc.channel_id).await? else {
        let ticket = new_ticket(&state, &mc.author, mc.channel_id, &message, &components).await?;
        store_message_counterpart(&state.db, ticket.channels, ticket.forum_post, mc.id).await?;
        return Ok(());
    };

    let message_response = state
        .client
        .create_message(channels.thread)
        .content(&message)
        .components(&components)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await;

    let message_response = match message_response {
        Ok(v) => Ok(v.model().await?),
        Err(e) => Err(e),
    };

    // This error handling exists in case the database gets into a bad state. We don't want to report
    // verbose errors to users for security reasons, so instead we give recreating the message a shot.
    let (channels, sent_message) = match message_response
        .as_ref()
        .map_err(twilight_http::Error::kind)
    {
        // Return our fancy new ticket if we needed to make one
        Err(twilight_http::error::ErrorType::Response { status, .. }) if status.get() == 404 => {
            delete_ticket(&state.db, channels.thread).await?;
            // Now that we've cleaned up our bad state, try to restart the interaction
            let ticket =
                new_ticket(&state, &mc.author, mc.channel_id, &message, &components).await?;
            Ok((ticket.channels, ticket.forum_post))
        }
        // Just map the old response into a ticket
        _ => message_response.map(|v| (channels, v.id)),
    }?;

    store_message_counterpart(&state.db, channels, sent_message, mc.id).await?;
    message_ack(&state.client, mc.channel_id, mc.id).await?;
    Ok(())
}

async fn message_ack(
    client: &Client,
    channel: Id<ChannelMarker>,
    message: Id<MessageMarker>,
) -> Result<(), Error> {
    client
        .create_reaction(
            channel,
            message,
            &RequestReactionType::Unicode { name: "✅" },
        )
        .await?;
    Ok(())
}

async fn new_ticket(
    state: &AppState,
    author: &User,
    dm_id: Id<ChannelMarker>,
    content: &str,
    components: &[Component],
) -> Result<NewTicket, Error> {
    let new_post = state
        .client
        .create_forum_thread(state.channel, &author.name)
        .message()
        .content(content)
        .components(components)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?
        .model()
        .await?;
    add_ticket(&state.db, new_post.channel.id, dm_id).await?;
    let _ = state
        .client
        .create_message(dm_id)
        .content(&state.open_message)
        .await
        .inspect_err(report_inspect);
    let channels = TicketChannels {
        thread: new_post.channel.id,
        dm: dm_id,
    };
    Ok(NewTicket {
        channels,
        forum_post: new_post.message.id,
    })
}

async fn handle_thread_delete(state: AppState, thread: Id<ChannelMarker>) {
    let Ok(Some(ticket)) = get_ticket(&state.db, thread)
        .await
        .inspect_err(report_inspect)
    else {
        return;
    };
    let _ = delete_ticket(&state.db, ticket.thread)
        .await
        .inspect_err(report_inspect);
    let _ = state
        .client
        .create_message(ticket.dm)
        .content(&state.close_message)
        .await
        .inspect_err(report_inspect);
}

async fn add_ticket(
    db: &SqlitePool,
    thread: Id<ChannelMarker>,
    dm: Id<ChannelMarker>,
) -> Result<(), sqlx::Error> {
    let thread = id_to_db(thread);
    let dm = id_to_db(dm);
    query!(
        "INSERT INTO tickets (dm, thread) VALUES (?1, ?2)",
        dm,
        thread
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Because snowflakes are unique, you can load either a dm or a thread into here, and it will
/// pop both right out.
async fn get_ticket(
    db: &SqlitePool,
    channel: Id<ChannelMarker>,
) -> Result<Option<TicketChannels>, sqlx::Error> {
    let channel = id_to_db(channel);
    let info = query!(
        "SELECT thread, dm FROM tickets WHERE dm = ?1 OR thread = ?1",
        channel
    )
    .fetch_optional(db)
    .await?
    .map(|row| TicketChannels {
        dm: db_to_id(row.dm),
        thread: db_to_id(row.thread),
    });

    Ok(info)
}

async fn delete_ticket(db: &SqlitePool, thread: Id<ChannelMarker>) -> Result<(), sqlx::Error> {
    let db_thread = id_to_db(thread);
    query!("DELETE FROM tickets WHERE thread = ?1", db_thread)
        .execute(db)
        .await?;
    Ok(())
}

async fn store_message_counterpart(
    db: &SqlitePool,
    ticket_channels: TicketChannels,
    thread_message: Id<MessageMarker>,
    dm_message: Id<MessageMarker>,
) -> Result<(), Error> {
    let thread_channel = id_to_db(ticket_channels.thread);
    let dm_channel = id_to_db(ticket_channels.dm);
    let thread_message = id_to_db(thread_message);
    let dm_message = id_to_db(dm_message);
    query!(
        "INSERT INTO ticket_messages (dm_channel, thread_channel, dm_message, thread_message) \
            VALUES (?1, ?2, ?3, ?4)",
        dm_channel,
        thread_channel,
        dm_message,
        thread_message
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn get_counterpart(
    db: &SqlitePool,
    message: Id<MessageMarker>,
) -> Result<Option<CounterpartedMessage>, Error> {
    let message = id_to_db(message);
    let row = query!("SELECT dm_channel, thread_channel, dm_message, thread_message FROM ticket_messages WHERE dm_message = ?1 OR thread_message = ?1", message).fetch_optional(db).await?;
    row.map_or(Ok(None), |row| {
        let thread = ChannelMessage {
            message: db_to_id(row.thread_message),
            channel: db_to_id(row.thread_channel),
        };
        let dm = ChannelMessage {
            message: db_to_id(row.dm_message),
            channel: db_to_id(row.dm_channel),
        };
        Ok(Some(CounterpartedMessage { thread, dm }))
    })
}

#[inline]
const fn id_to_db<T>(id: Id<T>) -> i64 {
    #[allow(clippy::cast_possible_wrap)]
    {
        id.get() as i64
    }
}

#[inline]
const fn db_to_id<T>(id: i64) -> Id<T> {
    #[allow(clippy::cast_sign_loss)]
    Id::new(id as u64)
}

fn attachments_to_components(attachments: Vec<Attachment>) -> Vec<Component> {
    let mut output: Vec<Vec<Attachment>> = Vec::new();
    let mut current: Vec<Attachment> = Vec::new();
    for attachment in attachments {
        if current.len() >= 5 {
            let finished = std::mem::take(&mut current);
            output.push(finished);
        }
        current.push(attachment);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output.into_iter().map(link_button_row).collect()
}

async fn attachments_to_attachments(
    attachments: &[Attachment],
) -> (Vec<HttpAttachment>, Vec<Error>) {
    let attachment_attempts =
        futures_util::future::join_all(attachments.iter().map(attachment_to_attachment)).await;
    let mut attachments = Vec::with_capacity(attachments.len());
    let mut errors = Vec::new();
    for attachment_attempt in attachment_attempts {
        match attachment_attempt {
            Ok(a) => attachments.push(a),
            Err(e) => errors.push(e),
        }
    }
    (attachments, errors)
}

async fn attachment_to_attachment(attachment: &Attachment) -> Result<HttpAttachment, Error> {
    let client = reqwest::Client::new();
    let body = client.get(&attachment.url).send().await?.bytes().await?;
    Ok(HttpAttachment {
        description: attachment.description.clone(),
        filename: attachment.filename.clone(),
        id: attachment.id.get(),
        file: body.to_vec(),
    })
}

fn link_button_row(attachments: Vec<Attachment>) -> Component {
    let components = attachments.into_iter().map(attachment_button).collect();
    Component::ActionRow(ActionRow { components })
}

fn attachment_button(attachment: Attachment) -> Component {
    Component::Button(Button {
        custom_id: None,
        disabled: false,
        emoji: Some(ReactionType::Unicode {
            name: "🔗".to_owned(),
        }),
        label: Some(attachment.filename),
        style: ButtonStyle::Link,
        url: Some(attachment.url),
    })
}

fn report_inspect<E: Debug>(e: &E) {
    eprintln!("ERROR: {e:?}");
}

#[derive(Clone, Copy, Debug)]
struct NewTicket {
    channels: TicketChannels,
    forum_post: Id<MessageMarker>,
}

#[derive(Clone, Copy, Debug)]
struct TicketChannels {
    dm: Id<ChannelMarker>,
    thread: Id<ChannelMarker>,
}

#[derive(Clone, Copy, Debug)]
struct ChannelMessage {
    channel: Id<ChannelMarker>,
    message: Id<MessageMarker>,
}

#[derive(Clone, Copy, Debug)]
struct CounterpartedMessage {
    thread: ChannelMessage,
    dm: ChannelMessage,
}

#[derive(Clone, Debug)]
pub struct AppState {
    client: Arc<Client>,
    open_message: Arc<str>,
    close_message: Arc<str>,
    channel: Id<ChannelMarker>,
    guild: Id<GuildMarker>,
    bot_id: Id<UserMarker>,
    db: SqlitePool,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("{0}")]
    Http(#[from] twilight_http::Error),
    #[error("{0}")]
    CdnHttp(#[from] reqwest::Error),
    #[error("{0}")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    #[error("No counterpart for message")]
    NoCounterpart,
}
