use super::common;
use crate::{AppContext, AppHandler};
use async_trait::async_trait;
use tokio::process;

pub(crate) struct SCSSimHandler {
    wine2linux_process: Option<process::Child>,
    wine2linux_args: &'static [&'static str],
    process_markers: &'static [&'static str],
}

impl SCSSimHandler {
    pub(crate) fn ets2() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &["--from-wine", r"Local\SHSCSTelemetry|SHSCSTelemetry"],
            process_markers: &["eurotrucks2.exe"],
        }
    }

    pub(crate) fn ats() -> Self {
        Self {
            wine2linux_process: None,
            wine2linux_args: &["--from-wine", r"Local\SHSCSTelemetry|SHSCSTelemetry"],
            process_markers: &["amtrucks.exe"],
        }
    }
}

#[async_trait(?Send)]
impl AppHandler for SCSSimHandler {
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
