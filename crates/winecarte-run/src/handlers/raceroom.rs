use super::common;
use crate::{AppContext, AppHandler};
use async_trait::async_trait;
use tokio::process;

pub(crate) struct RaceRoomHandler {
    wine2linux_process: Option<process::Child>,
    process_markers: &'static [&'static str],
}

impl RaceRoomHandler {
    const WINE2LINUX_ARGS: [&'static str; 2] = ["--from-wine", "$R3E"];

    pub(crate) fn raceroom() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["RRRE.exe", "RRRE64.exe"],
        }
    }
}

#[async_trait(?Send)]
impl AppHandler for RaceRoomHandler {
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
