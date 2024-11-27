#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
use std::{fmt::Debug, sync::Arc};

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
    id::{
        marker::{ChannelMarker, GuildMarker},
        Id,
    },
    user::User,
};
use valk_utils::{get_var, parse_var};

const HELP_MESSAGE: &str =
    "`!r <message>` to reply to a ticket, `!close` to close a ticket, `!help` to show this message";

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

    let state = AppState {
        client: Arc::new(client),
        channel,
        guild,
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
            Err(e) => {
                eprintln!("ERROR: {e:?}");
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
        if let Some(ticket_user) = get_ticket_by_thread(&state.db, mc.channel_id).await? {
            handle_guild_message_error_report_wrapper(state, mc, ticket_user).await?;
        }
    } else if mc.guild_id.is_none() {
        handle_user_message(state, mc).await?;
    }
    Ok(())
}

async fn handle_message_update(state: AppState, mu: MessageUpdate) -> Result<(), Error> {
    let channel = if mu.guild_id.is_none() {
        get_ticket_by_dm(&state.db, mu.channel_id).await?
    } else {
        None
    };

    if let (Some(channel), Some(content)) = (channel, mu.content) {
        let message = format!("message edited: {content}");
        state
            .client
            .create_message(channel.thread)
            .content(&message)
            .await?;
    }
    Ok(())
}
async fn handle_guild_message_error_report_wrapper(
    state: AppState,
    mc: Message,
    ticket: TicketInfo,
) -> Result<(), Error> {
    let channel_id = mc.channel_id;
    let reply_id = mc.id;
    if let Err(e) = handle_guild_message(state.clone(), mc, ticket).await {
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
    ticket: TicketInfo,
) -> Result<(), Error> {
    if mc.content == "!close" {
        delete_ticket(&state.db, ticket.thread).await?;
        let msg = if let Err(e) = state
            .client
            .create_message(ticket.dm)
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
    } else if mc.content == "!help" {
        state
            .client
            .create_message(mc.channel_id)
            .reply(mc.id)
            .content(HELP_MESSAGE)
            .await?;
    } else if let Some(("!r", content)) = mc.content.split_once(' ') {
        state
            .client
            .create_message(ticket.dm)
            .content(content)
            .await?;
        state
            .client
            .create_reaction(
                mc.channel_id,
                mc.id,
                &RequestReactionType::Unicode { name: "✅" },
            )
            .await?;
    }
    Ok(())
}

async fn handle_user_message(state: AppState, mc: Message) -> Result<(), Error> {
    let components = attachments_to_components(mc.attachments.clone());
    let message = format!("<@{}>: {}", mc.author.id, mc.content);

    let Some(ticket) = get_ticket_by_dm(&state.db, mc.channel_id).await? else {
        new_ticket(&state, &mc.author, mc.channel_id, &message, &components).await?;
        return Ok(());
    };
    let message_response = state
        .client
        .create_message(ticket.thread)
        .content(&message)
        .components(&components)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await;

    // This error handling exists in case the database gets into a bad state. We don't want to report
    // verbose errors to users for security reasons, so instead we give recreating the message a shot.
    match message_response
        .as_ref()
        .map_err(twilight_http::Error::kind)
    {
        Err(twilight_http::error::ErrorType::Response { status, .. }) if status.get() == 404 => {
            delete_ticket(&state.db, ticket.thread).await?;
            // Now that we've cleaned up our bad state, try to restart the interaction
            new_ticket(&state, &mc.author, mc.channel_id, &message, &components).await?;
            Ok(())
        }
        _ => message_response.map(|_| ()).map_err(Into::into),
    }
}

async fn new_ticket(
    state: &AppState,
    author: &User,
    dm_id: Id<ChannelMarker>,
    content: &str,
    components: &[Component],
) -> Result<TicketInfo, Error> {
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
    Ok(TicketInfo {
        thread: new_post.channel.id,
        dm: dm_id,
    })
}

async fn handle_thread_delete(state: AppState, thread: Id<ChannelMarker>) {
    let Ok(Some(ticket)) = get_ticket_by_thread(&state.db, thread)
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

async fn get_ticket_by_dm(
    db: &SqlitePool,
    dm: Id<ChannelMarker>,
) -> Result<Option<TicketInfo>, sqlx::Error> {
    let db_dm = id_to_db(dm);
    let info = query!("SELECT thread FROM tickets WHERE dm = ?1", db_dm)
        .fetch_optional(db)
        .await?
        .map(|row| TicketInfo {
            dm,
            thread: db_to_id(row.thread),
        });

    Ok(info)
}

async fn get_ticket_by_thread(
    db: &SqlitePool,
    thread: Id<ChannelMarker>,
) -> Result<Option<TicketInfo>, sqlx::Error> {
    let db_thread = id_to_db(thread);
    let info = query!("SELECT dm FROM tickets WHERE thread = ?1", db_thread)
        .fetch_optional(db)
        .await?
        .map(|row| TicketInfo {
            dm: db_to_id(row.dm),
            thread,
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
struct TicketInfo {
    dm: Id<ChannelMarker>,
    thread: Id<ChannelMarker>,
}

#[derive(Clone, Debug)]
pub struct AppState {
    client: Arc<Client>,
    open_message: Arc<str>,
    close_message: Arc<str>,
    channel: Id<ChannelMarker>,
    guild: Id<GuildMarker>,
    db: SqlitePool,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("{0}")]
    Http(#[from] twilight_http::Error),
    #[error("{0}")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
}
