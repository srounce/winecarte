mod assetto_corsa;
mod common;
mod le_mans_ultimate;

use crate::{AppHandler, StartupError};

pub(crate) use assetto_corsa::AssettoCorsaHandler;
pub(crate) use le_mans_ultimate::LeMansUltimateHandler;

pub(crate) fn get_handler(appid: &str) -> Result<Box<dyn AppHandler>, StartupError> {
    match appid {
        "2399420" => Ok(Box::new(LeMansUltimateHandler::default())),
        "244210" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa())),
        "805550" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa_competizione())),
        "3058630" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa_evo())),
        _ => Err(StartupError::UnsupportedAppId(appid.to_string())),
    }
}
