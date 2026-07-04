use rust_i18n::set_locale;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Chinese,
    English,
}

impl Language {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Chinese => "中文",
            Self::English => "English",
        }
    }

    #[must_use]
    pub fn locale(self) -> &'static str {
        match self {
            Self::Chinese => "zh_CN",
            Self::English => "en_US",
        }
    }
}

pub fn set_language(language: Language) {
    set_locale(language.locale());
}

#[cfg(test)]
pub(crate) fn with_locale_lock<T>(run: impl FnOnce() -> T) -> T {
    static LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCALE_LOCK.lock().expect("locale test lock poisoned");
    run()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rust_i18n::t;
    use serde_yaml::Value;

    use super::{Language, set_language, with_locale_lock};

    const LOCALES: &[&str] = &["zh_CN", "en_US"];

    fn locale_keys() -> BTreeSet<String> {
        let Value::Mapping(document) =
            serde_yaml::from_str(include_str!("../locales/app.yml")).expect("valid locale YAML")
        else {
            panic!("locale YAML must be a mapping");
        };

        let mut keys = BTreeSet::new();
        for (key, value) in document {
            let Some(key) = key.as_str() else {
                panic!("locale key must be a string");
            };
            if key == "_version" {
                continue;
            }

            let Value::Mapping(translations) = value else {
                panic!("translation `{key}` must be a locale mapping");
            };
            for locale in LOCALES {
                let locale_key = Value::String((*locale).to_owned());
                let Some(value) = translations.get(&locale_key) else {
                    panic!("translation `{key}` must define `{locale}`");
                };
                let Some(value) = value.as_str() else {
                    panic!("translation `{key}` `{locale}` value must be a string");
                };
                assert!(
                    !value.trim().is_empty(),
                    "translation `{key}` `{locale}` must not be empty"
                );
            }
            keys.insert(key.to_owned());
        }

        keys
    }

    fn source_t_macro_keys() -> BTreeSet<String> {
        let sources = [
            include_str!("app.rs"),
            include_str!("app/pages.rs"),
            include_str!("app/status.rs"),
            include_str!("app/widgets.rs"),
        ];
        let mut keys = BTreeSet::new();
        for source in sources {
            let mut rest = source;
            while let Some(index) = rest.find("t!(\"") {
                let previous = rest[..index].chars().next_back();
                if previous.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    rest = &rest[index + 4..];
                    continue;
                }
                let after_open_quote = &rest[index + 4..];
                let Some(end) = after_open_quote.find('"') else {
                    panic!("unterminated t! key in GUI source");
                };
                keys.insert(after_open_quote[..end].to_owned());
                rest = &after_open_quote[end + 1..];
            }
        }
        keys
    }

    #[test]
    fn rust_i18n_uses_current_locale_for_t_macro() {
        with_locale_lock(|| {
            set_language(Language::Chinese);
            assert_eq!(t!("start"), "启动");
            set_language(Language::English);
            assert_eq!(t!("start"), "Start");
        });
    }

    #[test]
    fn locale_file_has_supported_locales_for_each_key() {
        let keys = locale_keys();

        assert!(keys.contains("start"));
        assert!(keys.contains("config.warning"));
        assert!(keys.len() > 100);
    }

    #[test]
    fn source_t_macro_keys_exist_in_locale_file() {
        let locale_keys = locale_keys();

        for key in source_t_macro_keys() {
            assert!(locale_keys.contains(&key), "missing locale key `{key}`");
        }
    }
}
