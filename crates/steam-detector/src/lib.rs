//! Steam installation and SteamID64 discovery helpers.

/// Steam app id for The Binding of Isaac: Rebirth.
pub const ISAAC_APP_ID: u32 = 250_900;

/// Returns the Steam URI used to ask Steam to launch Isaac.
pub fn isaac_launch_uri() -> String {
    format!("steam://rungameid/{ISAAC_APP_ID}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_launch_uri() {
        assert_eq!(isaac_launch_uri(), "steam://rungameid/250900");
    }
}
