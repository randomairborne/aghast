use std::{cmp::Ordering, str::FromStr};

use niloecl::{FromRequest, IntoResponse};
use twilight_interactions::command::CommandModel;
use twilight_model::{
    application::interaction::{Interaction, InteractionData, InteractionType},
    guild::PartialMember,
};

use crate::interact::ErrorReport;

pub struct NoNameInRpc;

fn get_custom_id_rpc(custom_id: &str) -> Result<(&str, Vec<&str>), NoNameInRpc> {
    let mut items_iter = custom_id.split(':');
    let name = items_iter.next().ok_or(NoNameInRpc)?;
    let args = items_iter.collect();
    Ok((name, args))
}

pub struct SlashCommand<T: CommandModel>(pub T);

impl<T: CommandModel, S: Send + Sync> FromRequest<S> for SlashCommand<T> {
    type Rejection = SlashCommandRejection;

    async fn from_request(req: &mut Interaction, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(data) = &req.data else {
            return Err(SlashCommandRejection::NoInteractionData);
        };
        let InteractionData::ApplicationCommand(data) = data else {
            return Err(SlashCommandRejection::WrongInteractionData(req.kind));
        };
        CommandModel::from_interaction((**data).clone().into())
            .map_err(Into::into)
            .map(SlashCommand)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SlashCommandRejection {
    #[error("Wrong type of interaction data")]
    WrongInteractionData(InteractionType),
    #[error("No interaction data")]
    NoInteractionData,
    #[error("Arguments could not be parsed")]
    CommandParse(#[from] twilight_interactions::error::ParseError),
}

impl IntoResponse for SlashCommandRejection {
    fn into_response(self) -> twilight_model::http::interaction::InteractionResponse {
        ErrorReport(self).into_response()
    }
}

pub struct ExtractMember(pub PartialMember);

impl<S: Sync> FromRequest<S> for ExtractMember {
    type Rejection = ExtractMemberError;

    async fn from_request(req: &mut Interaction, _: &S) -> Result<Self, Self::Rejection> {
        req.member.clone().map(Self).ok_or(ExtractMemberError)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Discord did not send a member on this interaction")]
pub struct ExtractMemberError;

impl IntoResponse for ExtractMemberError {
    fn into_response(self) -> twilight_model::http::interaction::InteractionResponse {
        ErrorReport(self).into_response()
    }
}

pub struct CidArgs<T: FromCidArgs>(pub T);

impl<T: FromCidArgs, S: Sync> FromRequest<S> for CidArgs<T> {
    type Rejection = FromCidArgsRejection;

    async fn from_request(req: &mut Interaction, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(data) = &req.data else {
            return Err(FromCidArgsRejection::NoInteractionData);
        };
        let id_str = match data {
            InteractionData::MessageComponent(mc) => &mc.custom_id,
            InteractionData::ModalSubmit(ms) => &ms.custom_id,
            _ => return Err(FromCidArgsRejection::WrongInteractionData(req.kind)),
        };
        let (_name, args) =
            get_custom_id_rpc(id_str).map_err(|_| FromCidArgsRejection::NoDataName)?;
        T::from_args(&args).map(CidArgs).map_err(Into::into)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FromCidArgsRejection {
    #[error("Wrong type of interaction data")]
    WrongInteractionData(InteractionType),
    #[error("No interaction data")]
    NoInteractionData,
    #[error("No name in data")]
    NoDataName,
    #[error("Arguments could not be parsed")]
    ArgParse(#[from] FromCidArgsError),
}

impl IntoResponse for FromCidArgsRejection {
    fn into_response(self) -> twilight_model::http::interaction::InteractionResponse {
        ErrorReport(self).into_response()
    }
}

pub trait FromCidArgs: Sized {
    fn from_args(args: &[&str]) -> Result<Self, FromCidArgsError>;
}

macro_rules! impl_from_cid_args {
    ($($ty:ident),*) => {
        impl<$($ty,)*> FromCidArgs for ($($ty,)*)
        where
            $($ty: FromStr,)*
            $($ty::Err: std::error::Error + 'static,)*
        {
            fn from_args(args: &[&str]) -> Result<Self, FromCidArgsError> {
                // stringify just serves to "use" the type. The string is unused.
                let arg_count = 0 $(+ { stringify!($ty); 1 })*;
                match args.len().cmp(&arg_count) {
                    Ordering::Less => return Err(FromCidArgsError::RequiredCustomIdArgMissing(arg_count - args.len())),
                    Ordering::Equal => {},
                    Ordering::Greater => return Err(FromCidArgsError::ExtraCustomIdArgs(arg_count, args.len())),
                }
                match args {
                    #[allow(non_snake_case)]
                    [$($ty,)*] => Ok(($($ty::from_str($ty).map_err(|e| FromCidArgsError::UnconvertibleArgs(Box::new(e)))?,)*)),
                    _ => return Err(FromCidArgsError::UnconvertibleArgs("impossible arg state".into()))
                }
            }
        }
    };
}

#[derive(Debug, thiserror::Error)]
pub enum FromCidArgsError {
    #[error("Arguments were not convertible. this is a bug")]
    UnconvertibleArgs(Box<dyn std::error::Error + 'static>),
    #[error("Missing positional arg {0} for custom ID RPC")]
    RequiredCustomIdArgMissing(usize),
    #[error("Got wrong number of arguments: {0}, expected {1}")]
    ExtraCustomIdArgs(usize, usize),
}

impl_from_cid_args!(T1);
impl_from_cid_args!(T1, T2);
impl_from_cid_args!(T1, T2, T3);
impl_from_cid_args!(T1, T2, T3, T4);
impl_from_cid_args!(T1, T2, T3, T4, T5);
impl_from_cid_args!(T1, T2, T3, T4, T5, T6);
impl_from_cid_args!(T1, T2, T3, T4, T5, T6, T7);
impl_from_cid_args!(T1, T2, T3, T4, T5, T6, T7, T8);
impl_from_cid_args!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_from_cid_args!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
