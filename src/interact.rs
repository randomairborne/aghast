use std::fmt::Display;

use niloecl::IntoResponse;
use twilight_model::{
    application::interaction::{Interaction, InteractionType},
    channel::message::{
        component::{TextInput, TextInputStyle},
        Component, MessageFlags,
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{marker::ChannelMarker, Id},
};
use twilight_util::builder::{embed::EmbedBuilder, InteractionResponseDataBuilder};

use crate::{extract::CidArgs, AppState};

pub struct ErrorReport<T: Display>(pub T);

impl<T: Display> IntoResponse for ErrorReport<T> {
    fn into_response(self) -> InteractionResponse {
        let embed = EmbedBuilder::new()
            .description(format!("{}", self.0))
            .build();
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

async fn app_command() -> PingPong {
    PingPong
}

async fn msg_component(
    CidArgs((target_channel,)): CidArgs<(Id<ChannelMarker>,)>,
) -> InteractionResponse {
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
        max_length: None,
        min_length: None,
        placeholder: Some(placeholder.into()),
        required: Some(true),
        style,
        value: None,
    }
}

pub fn modal_activate(target_channel: Id<ChannelMarker>) -> InteractionResponse {
    let components = [
        modal_input(
            "user_id",
            "User(s) you are reporting (please provide ID if you can)",
            "wumpus",
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
    .map(Component::TextInput)
    .to_vec();

    let data = InteractionResponseData {
        components: Some(components),
        custom_id: Some(format!("open_resp:{:X}", target_channel.get())),
        title: Some("ModMail Form".to_string()),
        ..Default::default()
    };

    InteractionResponse {
        kind: InteractionResponseType::Modal,
        data: Some(data),
    }
}
