use std::fmt::{Debug, Display};

use niloecl::{IntoResponse, ModalSubmit, State};
use twilight_interactions::command::{CommandModel, CreateCommand};
use twilight_model::{
    application::interaction::{Interaction, InteractionType},
    channel::message::{
        component::{
            ActionRow, Button, ButtonStyle, SelectMenu, SelectMenuType, TextInput, TextInputStyle,
        },
        AllowedMentions, Component, MessageFlags,
    },
    guild::Permissions,
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{marker::ChannelMarker, Id},
};
use twilight_util::builder::{
    embed::{EmbedBuilder, EmbedFieldBuilder},
    InteractionResponseDataBuilder,
};

use crate::{
    extract::{CidArgs, ExtractMember, SlashCommand, UserSelectMenu},
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
    /// Placeholder for the user select menu
    #[command(min_length = 1, max_length = 45)]
    select_placeholder: String,
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

    let user_select = Component::SelectMenu(SelectMenu {
        channel_types: None,
        custom_id: format!("open_form_user:{}", cmd.modmail_channel.get()),
        default_values: None,
        disabled: false,
        kind: SelectMenuType::User,
        max_values: None,
        min_values: None,
        options: None,
        placeholder: Some(cmd.select_placeholder),
    });
    let user_select_row = Component::ActionRow(ActionRow {
        components: vec![user_select],
    });

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
        .components(&[user_select_row, submit_button_row])
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

/// This is a const to allow the msg_component function to format
const EXAMPLE_MESSAGE_LINK: &str =
    "e.g. https://discord.com/channels/302094807046684672/768594508287311882/768594834231132222";

async fn msg_component(
    CidArgs((target_channel,)): CidArgs<(Id<ChannelMarker>,)>,
    usm: Option<UserSelectMenu>,
) -> Result<ModalResponse, InteractError> {
    let components = [
        TextInput {
            custom_id: "user".into(),
            label: "Username or ID of the user you wish to report".into(), // this cannot be made longer
            max_length: Some(1000),
            min_length: None,
            placeholder: Some("e.g. wumpus or 302094807046684672".into()),
            required: Some(true),
            style: TextInputStyle::Short,
            value: None,
        },
        TextInput {
            custom_id: "channel".into(),
            label: "Channel name".into(),
            max_length: Some(128),
            min_length: None,
            placeholder: Some("e.g. #minecraft".into()),
            required: Some(true),
            style: TextInputStyle::Short,
            value: None,
        },
        TextInput {
            custom_id: "message_link".into(),
            label: "Message link".into(),
            max_length: Some(128),
            min_length: None,
            placeholder: Some(EXAMPLE_MESSAGE_LINK.into()),
            required: Some(false),
            style: TextInputStyle::Paragraph,
            value: None,
        },
        TextInput {
            custom_id: "reason".into(),
            label: "Reason for reporting (what happened, in detail)".into(),
            max_length: Some(128),
            min_length: None,
            placeholder: Some("e.g. User is being overly rude".into()),
            required: Some(true),
            style: TextInputStyle::Paragraph,
            value: None,
        },
    ]
    .map(|c| {
        Component::ActionRow(ActionRow {
            components: vec![Component::TextInput(c)],
        })
    });
    let (custom_id, components) = if let Some(UserSelectMenu(users)) = usm {
        let Some(user) = users.first() else {
            return Err(InteractError::NoUser);
        };
        (
            format!("form_submit:{}:{}", target_channel.get(), user.id),
            components[1..].to_vec(),
        )
    } else {
        (
            format!("form_submit:{}", target_channel.get()),
            components.as_slice().to_vec(),
        )
    };
    let title = "ModMail Form".to_string();
    Ok(ModalResponse {
        title,
        custom_id,
        components,
    })
}

#[derive(serde::Deserialize)]
pub struct ModmailFormModal {
    user: String,
    message_link: String,
    channel: String,
    reason: String,
}

async fn modal_submit(
    State(state): State<AppState>,
    ExtractMember(member): ExtractMember,
    modal: ModalSubmit<ModmailFormModal>,
    CidArgs((target_channel,)): CidArgs<(Id<ChannelMarker>,)>,
) -> Result<InteractionResponse, InteractError> {
    let user = member.user.ok_or(InteractError::NoUser)?;

    let user_field = EmbedFieldBuilder::new("User", modal.data.user)
        .inline()
        .build();
    let channel_field = EmbedFieldBuilder::new("Channel", modal.data.channel)
        .inline()
        .build();
    let message_link_field =
        EmbedFieldBuilder::new("Message link", modal.data.message_link).build();
    let reason_field = EmbedFieldBuilder::new("Reason", modal.data.reason).build();

    let embed = EmbedBuilder::new()
        .field(user_field)
        .field(channel_field)
        .field(message_link_field)
        .field(reason_field)
        .build();

    state
        .client
        .create_message(target_channel)
        .content(&format!("Report from <@{}>", user.id))
        .embeds(&[embed])
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?;

    let data = InteractionResponseDataBuilder::new()
        .flags(MessageFlags::EPHEMERAL)
        .content("Thanks for making a report. A moderator will handle it as soon as possible.")
        .build();

    Ok(InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(data),
    })
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
    #[error("Discord did not send a user where they were required to")]
    NoUser,
}

impl IntoResponse for InteractError {
    fn into_response(self) -> InteractionResponse {
        ErrorReport(self).into_response()
    }
}
