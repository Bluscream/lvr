//! Steam integration module: VDF parsing, path resolution, launch options, and process management.

pub mod launch_options;
pub mod paths;
pub mod process;
pub mod vdf;

#[allow(unused_imports)]
pub use launch_options::{
    read_launch_options, read_vrchat_launch_options, write_launch_options,
    write_vrchat_launch_options,
};
#[allow(unused_imports)]
pub use paths::SteamPaths;
pub use process::{shutdown_steam, start_steam, steam_running};
#[allow(unused_imports)]
pub use vdf::{edit_vdf, find_value, replace_value};

pub const VRCHAT_APPID: &str = "438100";
