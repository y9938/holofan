//! Internationalization (i18n) support and embedded FormatJS catalogs.

use formatjs_intl::{Intl, IntlCache, MessageCatalog};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocaleInfo {
    pub tag: &'static str,
    pub name: &'static str,
    pub flag: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Language(&'static LocaleInfo);

impl Language {
    pub fn parse(tag: &str) -> Option<Self> {
        let locale = tag.parse::<icu_locale_core::Locale>().ok()?;
        let canonical = locale.to_string();
        let info = AVAILABLE_LOCALES
            .iter()
            .find(|candidate| candidate.tag == canonical)?;
        Some(Self(info))
    }

    pub fn available() -> Vec<Self> {
        AVAILABLE_LOCALES.iter().map(Self).collect()
    }

    pub fn from_locale_hint(value: &str) -> Option<Self> {
        let value = value
            .split('@')
            .next()?
            .split('.')
            .next()?
            .replace('_', "-");
        if value.eq_ignore_ascii_case("C") || value.eq_ignore_ascii_case("POSIX") {
            return None;
        }
        if let Some(language) = Self::parse(&value) {
            return Some(language);
        }
        let primary = value.split('-').next()?;
        AVAILABLE_LOCALES
            .iter()
            .find(|locale| {
                locale
                    .tag
                    .split('-')
                    .next()
                    .is_some_and(|tag| tag.eq_ignore_ascii_case(primary))
            })
            .map(Self)
    }

    pub fn process_default() -> Self {
        Self::from_locale_environment(
            ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .map(|key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())),
        )
    }

    pub fn from_locale_environment(values: impl IntoIterator<Item = Option<String>>) -> Self {
        for value in values
            .into_iter()
            .flatten()
            .map(|value| value.trim().to_owned())
        {
            if value.is_empty() {
                continue;
            }
            return Self::from_locale_hint(&value).unwrap_or_else(Self::configured_fallback);
        }
        Self::configured_fallback()
    }

    /// Locale configured as default fallback.
    pub fn configured_fallback() -> Self {
        Self(
            AVAILABLE_LOCALES
                .iter()
                .find(|locale| locale.tag == FALLBACK_LOCALE)
                .expect("configured fallback locale is registered"),
        )
    }

    pub fn source() -> Self {
        Self(
            AVAILABLE_LOCALES
                .iter()
                .find(|locale| locale.tag == SOURCE_LOCALE)
                .expect("build-time source locale is registered"),
        )
    }

    pub const fn bcp47(self) -> &'static str {
        self.0.tag
    }

    pub const fn display_name(self) -> &'static str {
        self.0.name
    }

    pub const fn flag(self) -> Option<&'static str> {
        self.0.flag
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslationDescriptor {
    default_message: String,
}

type CatalogResult = Result<Arc<MessageCatalog>, &'static str>;

static CATALOG: LazyLock<CatalogResult> = LazyLock::new(|| {
    let mut catalog = MessageCatalog::new();
    // The source text is already present in each FormatJS descriptor. Keep the
    // source locale available to Intl without embedding a duplicate source catalog.
    catalog
        .insert(SOURCE_LOCALE, HashMap::new())
        .map_err(|_| "translation_catalog_invalid")?;
    for (locale, json) in EMBEDDED_CATALOGS {
        let extracted: HashMap<String, TranslationDescriptor> =
            serde_json::from_str(json).map_err(|_| "translation_catalog_invalid")?;
        catalog
            .insert(
                *locale,
                extracted
                    .into_iter()
                    .map(|(id, descriptor)| (id, descriptor.default_message))
                    .collect(),
            )
            .map_err(|_| "translation_catalog_invalid")?;
    }
    Ok(Arc::new(catalog))
});

static EMPTY_CATALOG: LazyLock<Arc<MessageCatalog>> = LazyLock::new(|| {
    let mut catalog = MessageCatalog::new();
    for locale in AVAILABLE_LOCALES {
        catalog
            .insert(locale.tag, HashMap::new())
            .expect("build-time locale registry is valid");
    }
    Arc::new(catalog)
});

static CACHE: LazyLock<Arc<IntlCache>> = LazyLock::new(|| Arc::new(IntlCache::new()));

pub fn for_language(language: Language) -> Intl {
    let catalog = match CATALOG.as_ref() {
        Ok(catalog) => catalog.clone(),
        Err(error) => {
            eprintln!("translation_catalog_invalid_using_source_fallback: {error}");
            EMPTY_CATALOG.clone()
        }
    };
    Intl::try_new([language.bcp47()], SOURCE_LOCALE, catalog, CACHE.clone())
        .expect("build-time locale tags are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_bcp47_tags_are_normalized_and_limited_to_embedded_catalogs() {
        assert_eq!(Language::parse("en"), Some(Language::source()));
        assert_eq!(Language::parse("EN"), Some(Language::source()));
        for language in Language::available() {
            assert_eq!(Language::parse(language.bcp47()), Some(language));
        }
        assert_eq!(Language::parse("xx"), None);
        assert_eq!(Language::parse("not a tag"), None);
    }

    #[test]
    fn every_registered_locale_has_a_catalog_and_display_name() {
        let languages = Language::available();
        assert_eq!(languages.len(), EMBEDDED_CATALOGS.len() + 1);
        assert!(EMBEDDED_CATALOGS
            .iter()
            .all(|(locale, _)| *locale != SOURCE_LOCALE));
        assert!(languages
            .iter()
            .all(|language| !language.display_name().is_empty()));
        assert!(languages
            .iter()
            .any(|language| language.bcp47() == SOURCE_LOCALE));
        assert!(languages
            .iter()
            .any(|language| language.bcp47() == FALLBACK_LOCALE));
    }

    #[test]
    fn process_locale_hints_resolve_without_language_specific_branches() {
        assert_eq!(
            Language::from_locale_hint("ru_RU.UTF-8"),
            Language::parse("ru")
        );
        assert_eq!(
            Language::from_locale_hint("en_US.UTF-8"),
            Language::parse("en")
        );
        assert_eq!(
            Language::from_locale_hint("zh_CN.UTF-8"),
            Language::parse("zh")
        );
        assert_eq!(Language::from_locale_hint("C.UTF-8"), None);
        assert_eq!(Language::from_locale_hint("unknown_LOCALE"), None);
    }

    #[test]
    fn process_locale_environment_obeys_first_nonempty_override() {
        assert_eq!(
            Language::from_locale_environment([
                Some("ru_RU.UTF-8".into()),
                Some("en_US.UTF-8".into()),
            ]),
            Language::parse("ru").unwrap()
        );
        assert_eq!(
            Language::from_locale_environment(
                [Some("C.UTF-8".into()), Some("en_US.UTF-8".into()),]
            ),
            Language::configured_fallback()
        );
    }

    #[test]
    fn for_language_returns_working_intl_instance() {
        for language in Language::available() {
            let intl = for_language(language);
            let rendered = formatjs_intl::format_message!(
                &intl,
                default_message: "Settings",
                description: "Settings screen title"
            );
            assert_eq!(rendered, "Settings");
        }
    }

    #[test]
    fn test_russian_catalog_lookup() {
        let intl = for_language(Language::parse("ru").unwrap());
        let rendered = formatjs_intl::format_message!(
            &intl,
            default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
            description: "Initial status bar message on application startup"
        );
        assert_eq!(
            rendered,
            "Готово к работе. Добавьте изображение, GIF, WebP или откройте файл контейнера (.bin)."
        );
    }

    #[test]
    fn test_chinese_catalog_lookup() {
        let intl = for_language(Language::parse("zh").unwrap());
        let rendered = formatjs_intl::format_message!(
            &intl,
            default_message: "Ready. Add an image, GIF, WebP, or open a container file (.bin).",
            description: "Initial status bar message on application startup"
        );
        assert_eq!(rendered, "就绪。添加图像、GIF、WebP或打开容器文件 (.bin)。");
    }
}
