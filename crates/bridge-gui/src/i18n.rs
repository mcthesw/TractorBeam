#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Chinese,
    English,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chinese => "中文",
            Self::English => "English",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Text {
    Home,
    Diagnostics,
    Debug,
    RelayHost,
    RelayPort,
    Room,
    Mode,
    SteamAccount,
    ManualSteamId,
    DisplayName,
    RefreshAccounts,
    Manual,
    Start,
    Stop,
    StartFailed,
    Close,
    Status,
    Idle,
    Running,
    Counters,
    HookToRelay,
    RelayToHook,
    SentBytes,
    ReceivedBytes,
    Errors,
    Logs,
    ExportDiagnostics,
    LastExport,
    RunRelayProbe,
    RelayProbePayloadBytes,
    LastRelayProbe,
    RunHookReceiveProbe,
    LastHookReceiveProbe,
    NoSteamAccounts,
    Official,
    Fallback,
    Pure,
    ConfigError,
}

pub fn text(language: Language, key: Text) -> &'static str {
    match language {
        Language::Chinese => zh(key),
        Language::English => en(key),
    }
}

fn zh(key: Text) -> &'static str {
    match key {
        Text::Home => "首页",
        Text::Diagnostics => "日志",
        Text::Debug => "调试",
        Text::RelayHost => "Relay 地址",
        Text::RelayPort => "端口",
        Text::Room => "房间",
        Text::Mode => "模式",
        Text::SteamAccount => "Steam 账号",
        Text::ManualSteamId => "SteamID64",
        Text::DisplayName => "用户名",
        Text::RefreshAccounts => "刷新账号",
        Text::Manual => "手动填写",
        Text::Start => "启动",
        Text::Stop => "停止",
        Text::StartFailed => "启动失败",
        Text::Close => "关闭",
        Text::Status => "状态",
        Text::Idle => "未启动",
        Text::Running => "运行中",
        Text::Counters => "计数器",
        Text::HookToRelay => "Hook 到 Relay",
        Text::RelayToHook => "Relay 到 Hook",
        Text::SentBytes => "发送字节",
        Text::ReceivedBytes => "接收字节",
        Text::Errors => "错误",
        Text::Logs => "日志",
        Text::ExportDiagnostics => "导出诊断",
        Text::LastExport => "最近导出",
        Text::RunRelayProbe => "运行 Relay 探针",
        Text::RelayProbePayloadBytes => "Relay 探针字节",
        Text::LastRelayProbe => "最近 Relay 探针",
        Text::RunHookReceiveProbe => "运行 Hook 收包探针",
        Text::LastHookReceiveProbe => "最近 Hook 探针",
        Text::NoSteamAccounts => "未自动识别到 Steam 账号，可以手动填写。",
        Text::Official => "Official",
        Text::Fallback => "Fallback",
        Text::Pure => "Pure",
        Text::ConfigError => "配置错误",
    }
}

fn en(key: Text) -> &'static str {
    match key {
        Text::Home => "Home",
        Text::Diagnostics => "Diagnostics",
        Text::Debug => "Debug",
        Text::RelayHost => "Relay host",
        Text::RelayPort => "Port",
        Text::Room => "Room",
        Text::Mode => "Mode",
        Text::SteamAccount => "Steam account",
        Text::ManualSteamId => "SteamID64",
        Text::DisplayName => "Display name",
        Text::RefreshAccounts => "Refresh accounts",
        Text::Manual => "Manual",
        Text::Start => "Start",
        Text::Stop => "Stop",
        Text::StartFailed => "Start failed",
        Text::Close => "Close",
        Text::Status => "Status",
        Text::Idle => "Idle",
        Text::Running => "Running",
        Text::Counters => "Counters",
        Text::HookToRelay => "Hook to relay",
        Text::RelayToHook => "Relay to hook",
        Text::SentBytes => "Sent bytes",
        Text::ReceivedBytes => "Received bytes",
        Text::Errors => "Errors",
        Text::Logs => "Logs",
        Text::ExportDiagnostics => "Export diagnostics",
        Text::LastExport => "Last export",
        Text::RunRelayProbe => "Run relay probe",
        Text::RelayProbePayloadBytes => "Relay probe bytes",
        Text::LastRelayProbe => "Last relay probe",
        Text::RunHookReceiveProbe => "Run hook receive probe",
        Text::LastHookReceiveProbe => "Last hook probe",
        Text::NoSteamAccounts => "No Steam account was detected. You can enter it manually.",
        Text::Official => "Official",
        Text::Fallback => "Fallback",
        Text::Pure => "Pure",
        Text::ConfigError => "Configuration error",
    }
}
