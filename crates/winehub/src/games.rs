use anyhow::Context;
use std::path::PathBuf;

#[allow(dead_code)]
pub struct GameContext {
    pub pid: u32,
    pub exe_path: PathBuf,
    pub install_dir: PathBuf,
    pub bridge_dir: PathBuf,
    pub compat_data_path: Option<PathBuf>,
}

pub struct GameBridge {
    pub process_names: &'static [&'static str],
    pub from_linux_args: &'static [&'static str],
    pub setup: Option<fn(&GameContext) -> anyhow::Result<()>>,
    pub teardown: Option<fn(&GameContext) -> anyhow::Result<()>>,
}

pub static GAMES: &[GameBridge] = &[
    GameBridge {
        process_names: &["acs.exe"],
        from_linux_args: &[
            "--from-linux",
            r"acpmf_physics|Local\acpmf_physics",
            "--from-linux",
            r"acpmf_graphics|Local\acpmf_graphics",
            "--from-linux",
            r"acpmf_static|Local\acpmf_static",
            "--from-linux",
            r"acpmf_simhub_v2|Local\acpmf_simhub_v2",
            "--from-linux",
            r"acpmf_crewchief|Local\acpmf_crewchief",
            "--from-linux",
            r"acpmf_secondMonitor|Local\acpmf_secondMonitor",
        ],
        setup: Some(ac_setup),
        teardown: None,
    },
    GameBridge {
        process_names: &["AC2-Win64-Shipping.exe"],
        from_linux_args: &[
            "--from-linux",
            r"acpmf_physics|Local\acpmf_physics",
            "--from-linux",
            r"acpmf_graphics|Local\acpmf_graphics",
            "--from-linux",
            r"acpmf_static|Local\acpmf_static",
        ],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["AssettoCorsaEVO.exe"],
        from_linux_args: &[
            "--from-linux",
            r"acevo_pmf_static|Local\acevo_pmf_static",
            "--from-linux",
            r"acevo_pmf_physics|Local\acevo_pmf_physics",
            "--from-linux",
            r"acevo_pmf_graphics|Local\acevo_pmf_graphics",
        ],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["acr.exe"],
        from_linux_args: &[
            "--from-linux",
            r"acpmf_physics|Local\acpmf_physics",
            "--from-linux",
            r"acpmf_graphics|Local\acpmf_graphics",
            "--from-linux",
            r"acpmf_static|Local\acpmf_static",
        ],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["rFactor2.exe"],
        from_linux_args: &[
            "--from-linux",
            "$rFactor2SMMP_Telemetry$",
            "--from-linux",
            "$rFactor2SMMP_Scoring$",
            "--from-linux",
            "$rFactor2SMMP_Rules$",
            "--from-linux",
            "$rFactor2SMMP_MultiRules$",
            "--from-linux",
            "$rFactor2SMMP_ForceFeedback$",
            "--from-linux",
            "$rFactor2SMMP_Graphics$",
            "--from-linux",
            "$rFactor2SMMP_Extended$",
            "--from-linux",
            "$rFactor2SMMP_PitInfo$",
            "--from-linux",
            "$rFactor2SMMP_Weather$",
            "--from-linux",
            "$rFactor2SMMP_HWControl$",
            "--from-linux",
            "$rFactor2SMMP_WeatherControl$",
            "--from-linux",
            "$rFactor2SMMP_RulesControl$",
            "--from-linux",
            "$rFactor2SMMP_PluginControl$",
        ],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["Le Mans Ultimate.exe"],
        from_linux_args: &[
            "--from-linux",
            "LMU_Data",
            "--from-linux",
            "$rFactor2SMMP_Telemetry$",
            "--from-linux",
            "$rFactor2SMMP_Scoring$",
            "--from-linux",
            "$rFactor2SMMP_Rules$",
            "--from-linux",
            "$rFactor2SMMP_MultiRules$",
            "--from-linux",
            "$rFactor2SMMP_ForceFeedback$",
            "--from-linux",
            "$rFactor2SMMP_Graphics$",
            "--from-linux",
            "$rFactor2SMMP_Extended$",
            "--from-linux",
            "$rFactor2SMMP_PitInfo$",
            "--from-linux",
            "$rFactor2SMMP_Weather$",
            "--from-linux",
            "$rFactor2SMMP_HWControl$",
            "--from-linux",
            "$rFactor2SMMP_WeatherControl$",
            "--from-linux",
            "$rFactor2SMMP_RulesControl$",
            "--from-linux",
            "$rFactor2SMMP_PluginControl$",
        ],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["pCARS2AVX.exe"],
        from_linux_args: &["--from-linux", "$pcars2$"],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["AMS2AVX.exe"],
        from_linux_args: &["--from-linux", "$pcars2$"],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["eurotrucks2.exe"],
        from_linux_args: &["--from-linux", r"SHSCSTelemetry|Local\SHSCSTelemetry"],
        setup: None,
        teardown: None,
    },
    GameBridge {
        process_names: &["amtrucks.exe"],
        from_linux_args: &["--from-linux", r"SHSCSTelemetry|Local\SHSCSTelemetry"],
        setup: None,
        teardown: None,
    },
];

fn ac_setup(ctx: &GameContext) -> anyhow::Result<()> {
    let src = ctx.install_dir.join("content");
    let dst = ctx.bridge_dir.join("content");
    std::os::unix::fs::symlink(&src, &dst)
        .with_context(|| format!("failed to symlink {} to {}", src.display(), dst.display()))
}
