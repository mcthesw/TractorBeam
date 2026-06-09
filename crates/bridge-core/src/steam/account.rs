/// Steam account details discovered from `loginusers.vdf`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteamAccount {
    pub steam_id64: String,
    pub account_name: Option<String>,
    pub persona_name: Option<String>,
    pub most_recent: bool,
}

impl SteamAccount {
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.persona_name
            .as_deref()
            .or(self.account_name.as_deref())
            .unwrap_or("Steam user")
    }
}
