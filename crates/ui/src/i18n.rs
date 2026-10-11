//! Localisation via Fluent (English, German).

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    En,
    De,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::De];

    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "Deutsch",
        }
    }

    /// From `LANG`/`LC_ALL` style environment values.
    pub fn from_env() -> Self {
        let v = std::env::var("CUTROOM_LANG")
            .or_else(|_| std::env::var("LC_ALL"))
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        if v.starts_with("de") {
            Lang::De
        } else {
            Lang::En
        }
    }

    fn source(self) -> &'static str {
        match self {
            Lang::En => include_str!("../assets/i18n/en.ftl"),
            Lang::De => include_str!("../assets/i18n/de.ftl"),
        }
    }

    fn id(self) -> unic_langid::LanguageIdentifier {
        match self {
            Lang::En => unic_langid::langid!("en-US"),
            Lang::De => unic_langid::langid!("de-DE"),
        }
    }
}

pub struct I18n {
    lang: Lang,
    bundle: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

fn bundle(lang: Lang) -> FluentBundle<FluentResource> {
    let res = FluentResource::try_new(lang.source().to_string()).expect("valid FTL");
    let mut b = FluentBundle::new_concurrent(vec![lang.id()]);
    b.set_use_isolating(false);
    b.add_resource(res).expect("no duplicate ids");
    b
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        I18n {
            lang,
            bundle: bundle(lang),
            fallback: bundle(Lang::En),
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        for b in [&self.bundle, &self.fallback] {
            if let Some(p) = b.get_message(key).and_then(|m| m.value()) {
                let mut errs = Vec::new();
                return b.format_pattern(p, args, &mut errs).into_owned();
            }
        }
        key.to_string()
    }

    /// Translates `key`; returns the key itself when missing.
    pub fn t(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Translates with named arguments.
    pub fn ta(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut a = FluentArgs::new();
        for (k, v) in args {
            a.set(*k, v.to_string());
        }
        self.format(key, Some(&a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translations_resolve() {
        let en = I18n::new(Lang::En);
        let de = I18n::new(Lang::De);
        assert_eq!(en.t("menu-file"), "File");
        assert_eq!(de.t("menu-file"), "Datei");
        assert_eq!(
            de.ta("props-editing-by", &[("name", "Ana")]),
            "Ana bearbeitet diesen Clip"
        );
        assert_eq!(en.t("missing-key"), "missing-key");
    }

    #[test]
    fn german_covers_every_english_key() {
        let keys = |src: &str| -> Vec<String> {
            src.lines()
                .filter_map(|l| l.split_once(" = ").map(|(k, _)| k.trim().to_string()))
                .collect()
        };
        let en = keys(Lang::En.source());
        let de = keys(Lang::De.source());
        for k in &en {
            assert!(de.contains(k), "missing German translation for {k}");
        }
    }
}
