use super::common;
use crate::{AppContext, AppHandler};
use async_trait::async_trait;
use tokio::process;

pub(crate) struct AssettoCorsaHandler {
    wine2linux_process: Option<process::Child>,
    wine2linux_args: &'static [&'static str],
    process_markers: &'static [&'static str],
}

impl AssettoCorsaHandler {
    const AC_WINE2LINUX_ARGS: [&'static str; 12] = [
        "--from-wine",
        "acpmf_physics",
        "--from-wine",
        "acpmf_graphics",
        "--from-wine",
        "acpmf_static",
        "--from-wine",
        "acpmf_simhub_v2",
        "--from-wine",
        "acpmf_crewchief",
        "--from-wine",
        "acpmf_secondMonitor",
    ];

    const ACC_WINE2LINUX_ARGS: [&'static str; 6] = [
        "--from-wine",
        "acpmf_physics",
        "--from-wine",
        "acpmf_graphics",
        "--from-wine",
        "acpmf_static",
    ];

    const ACE_WINE2LINUX_ARGS: [&'static str; 6] = [
        "--from-wine",
        r"Local\acevo_pmf_static|acevo_pmf_static",
        "--from-wine",
        r"Local\acevo_pmf_physics|acevo_pmf_physics",
        "--from-wine",
        r"Local\acevo_pmf_graphics|acevo_pmf_graphics",
    ];

    const ACR_WINE2LINUX_ARGS: [&'static str; 6] = [
        "--from-wine",
        "acpmf_physics",
        "--from-wine",
        "acpmf_graphics",
        "--from-wine",
        "acpmf_static",
    ];

    pub(crate) fn assetto_corsa() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &Self::AC_WINE2LINUX_ARGS,
            process_markers: &[
                "acs.exe",
                "AssettoCorsa.exe",
                "Content Manager.exe",
                "Content Manager Safe.exe",
            ],
        }
    }

    pub(crate) fn assetto_corsa_competizione() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &Self::ACC_WINE2LINUX_ARGS,
            process_markers: &["AC2-Win64-Shipping.exe"],
        }
    }

    pub(crate) fn assetto_corsa_evo() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &Self::ACE_WINE2LINUX_ARGS,
            process_markers: &["AssettoCorsaEVO.exe"],
        }
    }

    pub(crate) fn assetto_corsa_rally() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &Self::ACR_WINE2LINUX_ARGS,
            process_markers: &["acr.exe"],
        }
    }
}

#[async_trait(?Send)]
impl AppHandler for AssettoCorsaHandler {
    fn on_start(&mut self, context: &AppContext) -> anyhow::Result<()> {
        common::launch_wine2linux(&mut self.wine2linux_process, context, self.wine2linux_args)
    }

    async fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        common::cleanup_wine2linux(&mut self.wine2linux_process).await
    }

    fn probe_game_process(&mut self, _context: &AppContext) -> anyhow::Result<bool> {
        common::game_is_alive(self.process_markers)
    }
}
