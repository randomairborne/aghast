use twilight_model::{
    application::interaction::Interaction,
    http::interaction::{InteractionResponse, InteractionResponseType},
};

use crate::AppState;

pub async fn handle_interaction(state: AppState, interaction: Interaction) -> InteractionResponse {
    match route(&interaction) {
        Handler::Ping => InteractionResponse {
            data: None,
            kind: InteractionResponseType::Pong,
        },
    }
}

pub fn route(interaction: &Interaction) -> Handler {
    Handler::Ping
}

pub enum Handler {
    Ping,
}
