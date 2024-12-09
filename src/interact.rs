use std::fmt::{Debug, Display};

use niloecl::{IntoResponse, State};
use twilight_interactions::command::{CommandModel, CreateCommand};
use twilight_model::{
    application::interaction::{Interaction, InteractionType},
    channel::message::{
        component::{ActionRow, Button, ButtonStyle, TextInput, TextInputStyle},
        Component, MessageFlags,
    },
    guild::Permissions,
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{marker::ChannelMarker, Id},
};
use twilight_util::builder::{embed::EmbedBuilder, InteractionResponseDataBuilder};

use crate::{
    extract::{CidArgs, SlashCommand},
    AppState,
};

#[derive(CommandModel, CreateCommand)]
#[command(
    name = "setup",
    desc = "Initialize the modmail form",
    dm_permission = false,
    default_permissions = "Self::permissions"
)]
pub struct SetupCommand {
    /// The message to send.
    #[command(min_length = 1, max_length = 2000)]
    message: String,
    /// The text to put on the button
    #[command(min_length = 1, max_length = 32)]
    button_msg: String,
    /// The channel to send the message in
    button_channel: Id<ChannelMarker>,
    /// The channel to create modmails in
    modmail_channel: Id<ChannelMarker>,
}

impl SetupCommand {
    const fn permissions() -> Permissions {
        Permissions::ADMINISTRATOR
    }
}

pub struct ErrorReport<T: Display + Debug>(pub T);

impl<T: Display + Debug> IntoResponse for ErrorReport<T> {
    fn into_response(self) -> InteractionResponse {
        eprint!("ERROR: {:?}", self.0);
        let embed = EmbedBuilder::new().description(self.0.to_string()).build();
        let data = InteractionResponseDataBuilder::new()
            .flags(MessageFlags::EPHEMERAL)
            .embeds([embed])
            .build();
        InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(data),
        }
    }
}

pub async fn handle_interaction(state: AppState, interaction: Interaction) -> InteractionResponse {
    match interaction.kind {
        InteractionType::ApplicationCommand => {
            niloecl::make_handler(app_command)(interaction, state).await
        }
        InteractionType::MessageComponent => {
            niloecl::make_handler(msg_component)(interaction, state).await
        }
        InteractionType::ModalSubmit => {
            niloecl::make_handler(modal_submit)(interaction, state).await
        }
        _ => PingPong.into_response(),
    }
}

async fn app_command(
    State(state): State<AppState>,
    SlashCommand(cmd): SlashCommand<SetupCommand>,
) -> Result<InteractionResponse, InteractError> {
    let embed = EmbedBuilder::new().description(cmd.message).build();
    let submit_button = Component::Button(Button {
        custom_id: Some(format!("open_form:{}", cmd.modmail_channel.get())),
        disabled: false,
        emoji: None,
        label: Some(cmd.button_msg),
        style: ButtonStyle::Success,
        url: None,
    });
    let submit_button_row = Component::ActionRow(ActionRow {
        components: vec![submit_button],
    });

    state
        .client
        .create_message(cmd.button_channel)
        .embeds(&[embed])
        .components(&[submit_button_row])
        .await?;

    let data = InteractionResponseDataBuilder::new()
        .flags(MessageFlags::EPHEMERAL)
        .content("Creating button message")
        .build();

    Ok(InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(data),
    })
}

async fn msg_component(CidArgs((target_channel,)): CidArgs<(Id<ChannelMarker>,)>) -> ModalResponse {
    modal_activate(target_channel)
}

async fn modal_submit() -> PingPong {
    PingPong
}

struct PingPong;

impl IntoResponse for PingPong {
    fn into_response(self) -> InteractionResponse {
        InteractionResponse {
            kind: InteractionResponseType::Pong,
            data: None,
        }
    }
}

fn modal_input(
    id: impl Into<String>,
    label: impl Into<String>,
    placeholder: impl Into<String>,
    style: TextInputStyle,
) -> TextInput {
    TextInput {
        custom_id: id.into(),
        label: label.into(),
        max_length: Some(1000),
        min_length: Some(15),
        placeholder: Some(placeholder.into()),
        required: Some(true),
        style,
        value: None,
    }
}

pub fn modal_activate(target_channel: Id<ChannelMarker>) -> ModalResponse {
    let components = [
        modal_input(
            "user_id",
            "Username or ID of the user you wish to report", // this cannot be made longer
            "wumpus or 302094807046684672",
            TextInputStyle::Short,
        ),
        modal_input(
            "message_link",
            "Message link",
            "https://discord.com/channels/302094807046684672/768594508287311882/768594834231132222",
            TextInputStyle::Short,
        ),
        modal_input(
            "channel",
            "Channel name",
            "#minecraft",
            TextInputStyle::Short,
        ),
        modal_input(
            "reason",
            "Reason for reporting (what happened?)",
            "User is being overly rude",
            TextInputStyle::Paragraph,
        ),
    ]
    .map(|c| {
        Component::ActionRow(ActionRow {
            components: vec![Component::TextInput(c)],
        })
    })
    .to_vec();
    let custom_id = format!("form_submit:{}", target_channel.get());
    let title = "ModMail Form".to_string();
    ModalResponse {
        title,
        custom_id,
        components,
    }
}

#[derive(Debug, Clone)]
pub struct ModalResponse {
    title: String,
    custom_id: String,
    components: Vec<Component>,
}

impl IntoResponse for ModalResponse {
    fn into_response(self) -> InteractionResponse {
        let data = InteractionResponseDataBuilder::new()
            .title(self.title)
            .custom_id(self.custom_id)
            .components(self.components)
            .build();
        InteractionResponse {
            kind: InteractionResponseType::Modal,
            data: Some(data),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum InteractError {
    #[error("HTTP error: {0}")]
    Http(#[from] twilight_http::Error),
}

impl IntoResponse for InteractError {
    fn into_response(self) -> InteractionResponse {
        ErrorReport(self).into_response()
    }
}
