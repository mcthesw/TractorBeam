//! Steam installation, Isaac launch, and SteamID64 discovery helpers.

mod account;
mod paths;
mod registry;
mod vdf;

pub use account::SteamAccount;
pub use paths::{
    ISAAC_APP_ID, ISAAC_DEFAULT_INSTALL_DIR, detect_accounts, isaac_install_candidates,
    isaac_launch_uri, launch_isaac, loginusers_candidates, steam_install_candidates,
    steam_library_candidates,
};
pub use vdf::{parse_appmanifest_install_dir, parse_libraryfolders, parse_loginusers};
