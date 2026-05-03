use super::common;
use async_trait::async_trait;
use crate::{AppContext, AppHandler};
use tokio::process;

pub(crate) struct ProjectCars2Handler {
    wine2linux_process: Option<process::Child>,
    process_markers: &'static [&'static str],
}

impl ProjectCars2Handler {
    const WINE2LINUX_ARGS: [&'static str; 2] = ["--from-wine", "$pcars2$"];

    pub(crate) fn project_cars_2() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["pCARS2AVX.exe"],
        }
    }

    pub(crate) fn automobilista_2() -> Self {
        Self {
            wine2linux_process: None,
            process_markers: &["AMS2AVX.exe"],
        }
    }
}

#[async_trait(?Send)]
impl AppHandler for ProjectCars2Handler {
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
