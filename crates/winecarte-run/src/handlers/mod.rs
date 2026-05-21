mod assetto_corsa;
mod common;
mod le_mans_ultimate;
mod project_cars2;
mod scs_sim;

use crate::{AppHandler, StartupError};

pub(crate) use assetto_corsa::AssettoCorsaHandler;
pub(crate) use common::RunnerState;
pub(crate) use le_mans_ultimate::LeMansUltimateHandler;
pub(crate) use project_cars2::ProjectCars2Handler;
pub(crate) use scs_sim::SCSSimHandler;

pub(crate) fn get_handler(appid: &str) -> Result<Box<dyn AppHandler>, StartupError> {
    match appid {
        "2399420" => Ok(Box::new(LeMansUltimateHandler::le_mans_ultimate())),
        "365960" => Ok(Box::new(LeMansUltimateHandler::rfactor2())),
        "244210" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa())),
        "805550" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa_competizione())),
        "3058630" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa_evo())),
        "3917090" => Ok(Box::new(AssettoCorsaHandler::assetto_corsa_rally())),
        "378860" => Ok(Box::new(ProjectCars2Handler::project_cars_2())),
        "1066890" => Ok(Box::new(ProjectCars2Handler::automobilista_2())),
        "227300" => Ok(Box::new(SCSSimHandler::ets2())),
        "270880" => Ok(Box::new(SCSSimHandler::ats())),
        _ => Err(StartupError::UnsupportedAppId(appid.to_string())),
    }
}
