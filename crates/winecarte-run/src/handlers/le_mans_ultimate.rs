use super::common;
use crate::{AppContext, AppHandler};
use tokio::process;

pub(crate) struct LeMansUltimateHandler {
    wine2linux_process: Option<process::Child>,
}

impl Default for LeMansUltimateHandler {
    fn default() -> Self {
        Self {
            wine2linux_process: None,
        }
    }
}

impl LeMansUltimateHandler {
    const WINE2LINUX_ARGS: [&'static str; 30] = [
        "--from-wine",
        "LMU_Data",
        "--event",
        "LMU_Data_Event",
        "--from-wine",
        "$rFactor2SMMP_Telemetry$",
        "--from-wine",
        "$rFactor2SMMP_Scoring$",
        "--from-wine",
        "$rFactor2SMMP_Rules$",
        "--from-wine",
        "$rFactor2SMMP_MultiRules$",
        "--from-wine",
        "$rFactor2SMMP_ForceFeedback$",
        "--from-wine",
        "$rFactor2SMMP_Graphics$",
        "--from-wine",
        "$rFactor2SMMP_Extended$",
        "--from-wine",
        "$rFactor2SMMP_PitInfo$",
        "--from-wine",
        "$rFactor2SMMP_Weather$",
        "--from-wine",
        "$rFactor2SMMP_HWControl$",
        "--from-wine",
        "$rFactor2SMMP_WeatherControl$",
        "--from-wine",
        "$rFactor2SMMP_RulesControl$",
        "--from-wine",
        "$rFactor2SMMP_PluginControl$",
    ];
}

impl AppHandler for LeMansUltimateHandler {
    fn on_start(&mut self, context: &AppContext) -> anyhow::Result<()> {
        common::launch_wine2linux(
            &mut self.wine2linux_process,
            context,
            &Self::WINE2LINUX_ARGS,
        )
    }

    fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        common::cleanup_wine2linux(&mut self.wine2linux_process)
    }

    fn probe_game_process(&mut self, _context: &AppContext) -> anyhow::Result<bool> {
        common::game_is_alive(&["Le Mans Ultimate.exe"])
    }
}
