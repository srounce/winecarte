use super::common;
use async_trait::async_trait;
use crate::{AppContext, AppHandler};
use tokio::process;

pub(crate) struct AssettoCorsaHandler {
    wine2linux_process: Option<process::Child>,
    process_markers: &'static [&'static str],
}

impl AssettoCorsaHandler {
    const WINE2LINUX_ARGS: [&'static str; 6] = [
        "--from-wine",
        r"acpmf_physics",
        "--from-wine",
        r"acpmf_graphics",
        "--from-wine",
        r"acpmf_static",
    ];

    pub(crate) fn assetto_corsa() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["acs.exe", "AssettoCorsa.exe", "Content Manager.exe", "Content Manager Safe.exe"],
        }
    }

    pub(crate) fn assetto_corsa_competizione() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["AC2-Win64-Shipping.exe"],
        }
    }

    pub(crate) fn assetto_corsa_evo() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["AssettoCorsaEVO.exe"],
        }
    }
}

#[async_trait(?Send)]
impl AppHandler for AssettoCorsaHandler {
    fn on_start(&mut self, context: &AppContext) -> anyhow::Result<()> {
        common::launch_wine2linux(
            &mut self.wine2linux_process,
            context,
            &Self::WINE2LINUX_ARGS,
        )
    }

    async fn cleanup(&mut self, _context: &AppContext) -> anyhow::Result<()> {
        common::cleanup_wine2linux(&mut self.wine2linux_process).await
    }

    fn probe_game_process(&mut self, _context: &AppContext) -> anyhow::Result<bool> {
        common::game_is_alive(self.process_markers)
    }
}
