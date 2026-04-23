use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename = "libraryfolders")]
pub struct SteamLibraryFolders(HashMap<usize, LibraryFolder>);

#[derive(Serialize, Deserialize, Debug)]
pub struct LibraryFolder {
    path: String,
    label: String,
    #[serde(rename = "contentid")]
    content_id: String,
    #[serde(rename = "totalsize")]
    total_size: u64,
    update_clean_bytes_tally: u64,
    time_last_update_verified: u64,
    apps: HashMap<SteamAppId, u64>,
}

#[derive(Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub struct SteamAppId(String);


#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    #[test]
    fn it_works() {
        let result = SteamLibraryFolders(HashMap::from([
            (
                0,
                LibraryFolder {
                    path: "/mnt/games/SteamLibrary".into(),
                    label: "".into(),
                    content_id: "6998857582407221293".into(),
                    total_size: 2163350618112,
                    update_clean_bytes_tally: 2149040753,
                    time_last_update_verified: 1776708514,
                    apps: HashMap::from([
                        (SteamAppId("730".into()), 62290209783),
                        (SteamAppId("13620".into()), 444909094),
                        (SteamAppId("13630".into()), 541314280),
                        (SteamAppId("15300".into()), 1035618503),
                    ]),
                },
            ),
            (
                1,
                LibraryFolder {
                    path: "/mnt/fastgames/SteamLibrary".into(),
                    label: "".into(),
                    content_id: "6998857582407221293".into(),
                    total_size: 2163350618112,
                    update_clean_bytes_tally: 2149040753,
                    time_last_update_verified: 1776708514,
                    apps: HashMap::from([
                        (SteamAppId("244210".into()), 44949470831),
                        (SteamAppId("805550".into()), 20328954566),
                        (SteamAppId("1066890".into()), 142214751103),
                        (SteamAppId("1174180".into()), 128280821543),
                        (SteamAppId("1361210".into()), 98031503755),
                        (SteamAppId("1874880".into()), 27539838643),
                        (SteamAppId("2357570".into()), 70889563995),
                        (SteamAppId("2399420".into()), 46442708505),
                        (SteamAppId("3658110".into()), 1403717742),
                        (SteamAppId("3917090".into()), 20516889067),
                        (SteamAppId("4183110".into()), 645023192),
                    ]),
                }
            ),
        ]));
        let serialized = vdf_serde::to_string(&result).unwrap();
        println!("{result:#?}");
        println!("{serialized}");
        assert_eq!(serialized, "5");
    }

    #[test]
    fn it_really_works() {
        let home_dir_path = std::env::home_dir().unwrap();
        let home_dir = home_dir_path.to_str().unwrap();
        let input = read_to_string(format!("{}/.local/share/Steam/config/libraryfolders.vdf", home_dir)).unwrap();
        let deserialized: SteamLibraryFolders = vdf_serde::from_str(&input).unwrap();
        println!("{deserialized:#?}");

        let serialized = vdf_serde::to_string(&deserialized).unwrap();
        println!("{serialized}");
        assert_eq!(serialized, input);
    }
}
