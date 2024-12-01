#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(clippy::module_name_repetitions)]
use std::{fmt::Debug, future::IntoFuture, net::SocketAddr, pin::pin, sync::Arc};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hex::FromHex;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use twilight_http::Client;
use twilight_model::{
    application::interaction::Interaction, http::interaction::InteractionResponse,
};
use valk_utils::get_var;

mod extract;
mod interact;

fn main() {
    let token = get_var("AGHAST_TOKEN");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("aghast-main")
        .build()
        .unwrap();

    let client = Client::new(token);

    let cancel = CancellationToken::new();

    let bot_info = rt.block_on(async {
        client
            .current_user_application()
            .await
            .expect("Failed to get current user")
            .model()
            .await
            .expect("Failed to deserialize current user")
    });

    let key: [u8; ed25519_dalek::PUBLIC_KEY_LENGTH] =
        FromHex::from_hex(bot_info.verify_key).expect("Invalid signature hex");
    let key = VerifyingKey::from_bytes(&key).expect("Invalid signature bytes");

    let state = AppState {
        client: Arc::new(client),
        key,
    };

    let router = Router::new()
        .route("/api/interactions", post(interaction_handler))
        .with_state(state);

    let tcp = rt
        .block_on(TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8080))))
        .expect("Failed to bind to 8080");

    let mut main = rt.spawn(
        axum::serve(tcp, router)
            .with_graceful_shutdown(cancel.clone().cancelled_owned())
            .into_future(),
    );

    eprintln!("Event loop started");

    rt.block_on(async move {
        // we need to do all these things in this order so that we tell everything to shut down,
        // then wait for it to shut down, then clean up
        futures_util::future::select(pin!(vss::shutdown_signal()), pin!(&mut main)).await;
        eprintln!("Shutting down");
        cancel.cancel();
        main.await
            .expect("Server panicked")
            .expect("Could not start server");
    });
}

async fn interaction_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<InteractionResponse>, RequestError> {
    // Extract the timestamp header for use later to check the signature.
    let timestamp = headers
        .get("x-signature-timestamp")
        .ok_or(RequestError::BadSignature)?;

    // Extract the signature to check against.
    let signature: Signature = headers
        .get("x-signature-ed25519")
        .and_then(|v| v.to_str().ok())
        .ok_or(RequestError::BadSignature)?
        .parse()
        .map_err(|_| RequestError::BadSignature)?;

    let whole_body = [timestamp.as_bytes(), &body].concat();

    if state.key.verify(&whole_body, &signature).is_err() {
        return Err(RequestError::BadSignature);
    }

    let interaction: Interaction =
        serde_json::from_slice(&body).map_err(|_| RequestError::BadJson)?;
    Ok(Json(interact::handle_interaction(state, interaction).await))
}

#[derive(Clone, Debug)]
pub struct AppState {
    client: Arc<Client>,
    key: VerifyingKey,
}

enum RequestError {
    BadSignature,
    BadJson,
}

impl IntoResponse for RequestError {
    fn into_response(self) -> axum::response::Response {
        match &self {
            Self::BadSignature => (StatusCode::UNAUTHORIZED, "Bad signature or headers"),
            Self::BadJson => (StatusCode::BAD_REQUEST, "Bad JSON body"),
        }
        .into_response()
    }
}
