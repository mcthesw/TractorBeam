use std::borrow::Cow;

use eframe::egui;
use rust_i18n::t;
use tractor_beam_core::build_info;

use crate::app::BridgeApp;

const PROTOCOL_VERSION: &str = "2.0";
const ACKNOWLEDGEMENTS_TWO_COLUMN_MIN_WIDTH: f32 = 620.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContributorIdentity {
    Contributor,
    EarlyTester,
}

impl ContributorIdentity {
    fn label(self) -> Cow<'static, str> {
        match self {
            Self::Contributor => t!("about.identity.contributor"),
            Self::EarlyTester => t!("about.identity.early_tester"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Contributor {
    name: &'static str,
    url: Option<&'static str>,
    identity: ContributorIdentity,
}

const CONTRIBUTORS: &[Contributor] = &[
    Contributor {
        name: "Sworld",
        url: Some("https://github.com/mcthesw"),
        identity: ContributorIdentity::Contributor,
    },
    Contributor {
        name: "北国无人",
        url: None,
        identity: ContributorIdentity::Contributor,
    },
    Contributor {
        name: "其他匿名玩家",
        url: None,
        identity: ContributorIdentity::Contributor,
    },
    Contributor {
        name: "Summerraim",
        url: Some("https://github.com/Summerraim"),
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "勺子c",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "土拨鼠",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "老吴",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "紬",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "空弦弦娴",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "舟飏",
        url: Some("https://github.com/LLIittleFish"),
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "扣1跟科比打复活赛",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
    Contributor {
        name: "闲舒",
        url: None,
        identity: ContributorIdentity::EarlyTester,
    },
];

fn acknowledgements_columns(available_width: f32) -> usize {
    if available_width >= ACKNOWLEDGEMENTS_TWO_COLUMN_MIN_WIDTH {
        2
    } else {
        1
    }
}

fn contributor_name(ui: &mut egui::Ui, contributor: Contributor) {
    if let Some(url) = contributor.url {
        ui.hyperlink_to(contributor.name, url);
    } else {
        ui.strong(contributor.name);
    }
}

impl BridgeApp {
    pub(in crate::app) fn about_page(&mut self, ui: &mut egui::Ui) {
        let about_label = t!("about");
        let desc_label = t!("about.desc");
        let version_label = t!("version");
        let proto_label = t!("about.protocol_version");
        ui.heading(about_label);
        ui.add_space(12.0);
        ui.label(desc_label);
        ui.add_space(16.0);
        egui::Grid::new("about_grid")
            .num_columns(2)
            .spacing([20.0, 6.0])
            .show(ui, |ui| {
                ui.label(version_label);
                ui.monospace(build_info::version_label());
                ui.end_row();
                ui.label(proto_label);
                ui.monospace(PROTOCOL_VERSION);
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.hyperlink_to(
            t!("about.source_repository"),
            "https://github.com/mcthesw/TractorBeam",
        );
        ui.add_space(2.0);
        ui.label(format!("{}: GNU AGPL-3.0-or-later", t!("license")));
        ui.add_space(20.0);
        ui.separator();
        ui.add_space(12.0);
        ui.heading(t!("about.acknowledgements"));
        ui.label(t!("about.acknowledgements_desc"));
        ui.add_space(8.0);
        let columns = acknowledgements_columns(ui.available_width());
        egui::Grid::new("acknowledgements_grid")
            .num_columns(columns * 2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for row in CONTRIBUTORS.chunks(columns) {
                    for contributor in row.iter().copied() {
                        contributor_name(ui, contributor);
                        ui.weak(contributor.identity.label());
                    }
                    ui.end_row();
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgements_preserve_supplied_order_names_and_links() {
        assert_eq!(CONTRIBUTORS.len(), 12);
        assert_eq!(CONTRIBUTORS[0].name, "Sworld");
        assert_eq!(CONTRIBUTORS[1].name, "北国无人");
        assert_eq!(CONTRIBUTORS[2].name, "其他匿名玩家");
        assert_eq!(CONTRIBUTORS[9].name, "舟飏");
        assert_eq!(CONTRIBUTORS[0].url, Some("https://github.com/mcthesw"));
        assert_eq!(CONTRIBUTORS[3].url, Some("https://github.com/Summerraim"));
        assert_eq!(CONTRIBUTORS[9].url, Some("https://github.com/LLIittleFish"));
        assert!(
            CONTRIBUTORS
                .iter()
                .enumerate()
                .all(|(index, contributor)| [0, 3, 9].contains(&index)
                    || contributor.url.is_none())
        );
    }

    #[test]
    fn contributors_are_listed_before_early_testers() {
        assert!(
            CONTRIBUTORS[..3]
                .iter()
                .all(|contributor| contributor.identity == ContributorIdentity::Contributor)
        );
        assert!(
            CONTRIBUTORS[3..]
                .iter()
                .all(|contributor| contributor.identity == ContributorIdentity::EarlyTester)
        );
    }

    #[test]
    fn acknowledgements_use_two_columns_only_when_space_allows() {
        assert_eq!(acknowledgements_columns(619.0), 1);
        assert_eq!(acknowledgements_columns(620.0), 2);
    }
}
