/// Everything winecarte knows about one game, shared by both binaries so the
/// sender and receiver mapping lists cannot drift apart.
pub struct Game {
    /// Slug identifying the game. Names its persistent bridge directory.
    pub name: &'static str,
    pub steam_appids: &'static [&'static str],
    /// The exe(s) that publish telemetry. Detecting one of these starts a
    /// bridge.
    pub process_names: &'static [&'static str],
    /// Launcher exes that keep a Steam session alive before or after the game
    /// exe itself. winecarte-run treats these as "still running".
    pub launcher_names: &'static [&'static str],
    /// wine2linux args for the sender running inside the game's prefix.
    pub from_wine_args: &'static [&'static str],
    /// wine2linux args for the receiver running inside the SimHub prefix.
    pub from_linux_args: &'static [&'static str],
    /// Symlink every directory sitting alongside the game exe into the bridge
    /// dir, for games whose tools resolve data relative to the running exe.
    pub link_sibling_dirs: bool,
}

impl Game {
    pub fn matches_process(&self, argv0: &str) -> bool {
        self.process_names
            .iter()
            .any(|&m| crate::procscan::exe_matches(argv0, m))
    }
}

pub fn by_appid(appid: &str) -> Option<&'static Game> {
    GAMES.iter().find(|g| g.steam_appids.contains(&appid))
}

pub fn by_process(argv0: &str) -> Option<&'static Game> {
    GAMES.iter().find(|g| g.matches_process(argv0))
}

const RF2_SMMP_FROM_WINE: &[&str] = &[
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

const RF2_SMMP_FROM_LINUX: &[&str] = &[
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
];

const ACPMF_FROM_WINE: &[&str] = &[
    "--from-wine",
    "acpmf_physics",
    "--from-wine",
    "acpmf_graphics",
    "--from-wine",
    "acpmf_static",
];

const ACPMF_FROM_LINUX: &[&str] = &[
    "--from-linux",
    r"acpmf_physics|Local\acpmf_physics",
    "--from-linux",
    r"acpmf_graphics|Local\acpmf_graphics",
    "--from-linux",
    r"acpmf_static|Local\acpmf_static",
];

pub static GAMES: &[Game] = &[
    Game {
        name: "assetto-corsa",
        steam_appids: &["244210"],
        process_names: &["acs.exe"],
        launcher_names: &[
            "AssettoCorsa.exe",
            "Content Manager.exe",
            "Content Manager Safe.exe",
        ],
        from_wine_args: &[
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
        ],
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
        link_sibling_dirs: true,
    },
    Game {
        name: "assetto-corsa-competizione",
        steam_appids: &["805550"],
        process_names: &["AC2-Win64-Shipping.exe"],
        launcher_names: &[],
        from_wine_args: ACPMF_FROM_WINE,
        from_linux_args: ACPMF_FROM_LINUX,
        link_sibling_dirs: false,
    },
    Game {
        name: "assetto-corsa-evo",
        steam_appids: &["3058630"],
        process_names: &["AssettoCorsaEVO.exe"],
        launcher_names: &[],
        from_wine_args: &[
            "--from-wine",
            r"Local\acevo_pmf_static|acevo_pmf_static",
            "--from-wine",
            r"Local\acevo_pmf_physics|acevo_pmf_physics",
            "--from-wine",
            r"Local\acevo_pmf_graphics|acevo_pmf_graphics",
        ],
        from_linux_args: &[
            "--from-linux",
            r"acevo_pmf_static|Local\acevo_pmf_static",
            "--from-linux",
            r"acevo_pmf_physics|Local\acevo_pmf_physics",
            "--from-linux",
            r"acevo_pmf_graphics|Local\acevo_pmf_graphics",
        ],
        link_sibling_dirs: false,
    },
    Game {
        name: "assetto-corsa-rally",
        steam_appids: &["3917090"],
        process_names: &["acr.exe"],
        launcher_names: &[],
        from_wine_args: ACPMF_FROM_WINE,
        from_linux_args: ACPMF_FROM_LINUX,
        link_sibling_dirs: false,
    },
    Game {
        name: "rfactor2",
        steam_appids: &["365960"],
        process_names: &["rFactor2.exe"],
        launcher_names: &[],
        from_wine_args: RF2_SMMP_FROM_WINE,
        from_linux_args: RF2_SMMP_FROM_LINUX,
        link_sibling_dirs: false,
    },
    Game {
        name: "le-mans-ultimate",
        steam_appids: &["2399420"],
        process_names: &["Le Mans Ultimate.exe"],
        launcher_names: &[],
        from_wine_args: &[
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
        ],
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
        link_sibling_dirs: true,
    },
    Game {
        name: "project-cars-2",
        steam_appids: &["378860"],
        process_names: &["pCARS2AVX.exe"],
        launcher_names: &[],
        from_wine_args: &["--from-wine", "$pcars2$"],
        from_linux_args: &["--from-linux", "$pcars2$"],
        link_sibling_dirs: false,
    },
    Game {
        name: "automobilista-2",
        steam_appids: &["1066890"],
        process_names: &["AMS2.exe", "AMS2AVX.exe"],
        launcher_names: &[],
        from_wine_args: &["--from-wine", "$pcars2$"],
        from_linux_args: &["--from-linux", "$pcars2$"],
        link_sibling_dirs: true,
    },
    Game {
        name: "euro-truck-simulator-2",
        steam_appids: &["227300"],
        process_names: &["eurotrucks2.exe"],
        launcher_names: &[],
        from_wine_args: &["--from-wine", r"Local\SHSCSTelemetry|SHSCSTelemetry"],
        from_linux_args: &["--from-linux", r"SHSCSTelemetry|Local\SHSCSTelemetry"],
        link_sibling_dirs: false,
    },
    Game {
        name: "american-truck-simulator",
        steam_appids: &["270880"],
        process_names: &["amtrucks.exe"],
        launcher_names: &[],
        from_wine_args: &["--from-wine", r"Local\SHSCSTelemetry|SHSCSTelemetry"],
        from_linux_args: &["--from-linux", r"SHSCSTelemetry|Local\SHSCSTelemetry"],
        link_sibling_dirs: false,
    },
    Game {
        name: "raceroom",
        steam_appids: &["211500"],
        process_names: &["RRRE.exe", "RRRE64.exe"],
        launcher_names: &[],
        from_wine_args: &["--from-wine", "$R3E"],
        from_linux_args: &["--from-linux", r"$R3E|Local\$R3E"],
        link_sibling_dirs: false,
    },
    // The remaining games publish telemetry over UDP, which already reaches
    // the SimHub prefix, but SimHub only detects a game by a process running
    // there. Their receivers run wine2linux with --stand-in so SimHub sees
    // the game without bridging any mappings; there is nothing to send.
    Game {
        name: "dirt-rally-2",
        steam_appids: &[],
        process_names: &["dirtrally2.exe"],
        launcher_names: &[],
        from_wine_args: &[],
        from_linux_args: &["--stand-in"],
        link_sibling_dirs: false,
    },
    Game {
        name: "beamng-drive",
        steam_appids: &[],
        // The native Linux build is detected too; its stand-in gets `.exe`.
        process_names: &["BeamNG.drive.x64.exe", "BeamNG.drive.x64"],
        launcher_names: &[],
        from_wine_args: &[],
        from_linux_args: &["--stand-in"],
        link_sibling_dirs: false,
    },
    Game {
        name: "wreckfest-2",
        steam_appids: &[],
        process_names: &["Wreckfest2.exe"],
        launcher_names: &[],
        from_wine_args: &[],
        from_linux_args: &["--stand-in"],
        link_sibling_dirs: false,
    },
];
