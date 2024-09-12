//! Convenience builders for Inertia using [vitejs].
//!
//! This module provides [Development] and [Production] structs for
//! different environments, e.g.:
//!
//! ```rust
//! use axum_inertia::vite;
//!
//! // are we production?
//! let is_production = std::env::var("APP_ENV").map_or(false, |s| &s[..] == "production");
//!
//! let inertia = if is_production {
//!     vite::Production::new("client/dist/manifest.json", "src/main.ts")
//!         .unwrap()
//!         .lang("en")
//!         .title("My app")
//!         .into_config()
//! } else {
//!     vite::Development::default()
//!         .port(5173)
//!         .main("src/main.ts")
//!         .lang("en")
//!         .title("My app")
//!         .react() // call if using react
//!         .into_config()
//! };
//! ```
//!
//! [vitejs]: https://vitejs.dev
use crate::config::InertiaConfig;
use hex::encode;
use maud::{html, PreEscaped};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use tera::{Context as TeraContext, Tera};

pub struct Development {
    port: u16,
    main: &'static str,
    lang: &'static str,
    title: &'static str,
    react: bool,
    template_engine: Option<Tera>,
    layout_template: Option<String>,
}

impl Default for Development {
    fn default() -> Self {
        Development {
            port: 5173,
            main: "src/main.ts",
            lang: "en",
            title: "Vite",
            react: false,
            template_engine: None,
            layout_template: None,
        }
    }
}

impl Development {
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn main(mut self, main: &'static str) -> Self {
        self.main = main;
        self
    }

    pub fn lang(mut self, lang: &'static str) -> Self {
        self.lang = lang;
        self
    }

    pub fn title(mut self, title: &'static str) -> Self {
        self.title = title;
        self
    }

    /// Sets up vite for react usage.
    ///
    /// Currently, this will include preamble code for using react-refresh in the html head.
    /// Some context here: https://github.com/vitejs/vite/issues/1984
    pub fn react(mut self) -> Self {
        self.react = true;
        self
    }

    pub fn template_engine<T: AsRef<str>>(mut self, engine: Tera, layout_template: T) -> Self {
        self.template_engine = Some(engine);
        self.layout_template = Some(layout_template.as_ref().to_owned());

        self
    }

    pub fn into_config(self) -> InertiaConfig {
        let layout = Box::new(move |props| {
            if let Some(layout_template) = &self.layout_template {
                let mut context = TeraContext::new();

                let vite_client = html! { 
                    script type="module" src=(format!("http://localhost:{}/@vite/client", self.port)) {}
                }.into_string();
                context.insert(
                    "vite_client",
                    &vite_client
                );

                let vite_main = html! {
                    script type="module" src=(format!("http://localhost:{}/{}", self.port, self.main)) {}
                }.into_string();
                context.insert(
                    "vite_main",
                    &vite_main,
                );

                let react_preamble = html!{
                    script type="module" { (PreEscaped(self.build_react_preamble())) }
                }.into_string();
                context.insert("vite_react_refresh", &react_preamble);

                let app_element = html! {
                    div #app data-page=(props) {}
                }
                .into_string();
                context.insert("application", &app_element);

                match &self.template_engine {
                    Some(template_engine) => {
                        match template_engine.render(layout_template, &context) {
                            Ok(output) => output,
                            Err(err) => {
                                eprintln!("Failed to render template {err}");
                                "".to_string()
                            }
                        }
                    }
                    None => "".to_string(),
                }
            } else {
                let vite_src = format!("http://localhost:{}/@vite/client", self.port);
                let main_src = format!("http://localhost:{}/{}", self.port, self.main);
                let preamble_code = if self.react {
                    Some(PreEscaped(self.build_react_preamble()))
                } else {
                    None
                };
                html! {
                    html lang=(self.lang) {
                        head {
                            title { (self.title) }
                            meta charset="utf-8";
                            meta name="viewport" content="width=device-width, initial-scale=1.0";
                            @if let Some(preamble_code) = preamble_code {
                                script type="module" { (preamble_code) }
                            }
                            script type="module" src=(vite_src) {}
                            script type="module" src=(main_src) {}
                        }

                        body {
                            div #app data-page=(props) {}
                        }
                    }
                }
                .into_string()
            }
        });

        InertiaConfig::new(None, layout)
    }

    fn build_react_preamble(&self) -> String {
        format!(
            r#"
import RefreshRuntime from "http://localhost:{}/@react-refresh"
RefreshRuntime.injectIntoGlobalHook(window)
window.$RefreshReg$ = () => {{}}
window.$RefreshSig$ = () => (type) => type
window.__vite_plugin_react_preamble_installed__ = true
"#,
            self.port
        )
    }
}

pub struct Production {
    main: ManifestEntry,
    css: Option<String>,
    title: &'static str,
    lang: &'static str,
    /// SHA1 hash of the contents of the manifest file.
    version: String,
    template_engine: Option<Tera>,
    layout_template: Option<String>,
    asset_path: Option<String>,
}

impl Production {
    pub fn new(
        manifest_path: &'static str,
        main: &'static str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(manifest_path)?;

        Self::new_from_string(&String::from_utf8(bytes)?, main)
    }

    fn new_from_string(
        manifest_string: &str,
        main: &'static str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut manifest: HashMap<String, ManifestEntry> = serde_json::from_str(&manifest_string)?;
        let entry = manifest.remove(main).ok_or(ViteError::EntryMissing(main))?;
        let mut hasher = Sha1::new();
        hasher.update(manifest_string.as_bytes());
        let result = hasher.finalize();
        let version = encode(result);
        let css = {
            if let Some(css_sources) = &entry.css {
                let mut css = String::new();
                for source in css_sources {
                    css.push_str(&format!(r#"<link rel="stylesheet" href="/{source}"/>"#));
                }
                Some(css)
            } else {
                None
            }
        };
        Ok(Self {
            main: entry,
            css,
            title: "Vite",
            lang: "en",
            version,
            template_engine: None,
            layout_template: None,
            asset_path: None,
        })
    }

    pub fn lang(mut self, lang: &'static str) -> Self {
        self.lang = lang;
        self
    }

    pub fn title(mut self, title: &'static str) -> Self {
        self.title = title;
        self
    }

    pub fn template_engine<T: AsRef<str>>(mut self, engine: Tera, layout_template: T) -> Self {
        self.template_engine = Some(engine);
        self.layout_template = Some(layout_template.as_ref().to_owned());

        self
    }

    pub fn asset_path<P: AsRef<str>>(mut self, asset_path: P) -> Self {
        self.asset_path = Some(asset_path.as_ref().to_owned());

        self
    }

    pub fn into_config(self) -> InertiaConfig {
        let layout = Box::new(move |props| {
            let main_path = match &self.asset_path {
                Some(asset_path) => format!("/{}/{}", asset_path, self.main.file),
                None => format!("/{}", self.main.file),
            };
            let main_integrity = self.main.integrity.clone();

            if let Some(template_engine) = &self.template_engine {
                let mut context = TeraContext::new();

                context.insert("vite_client","");
                context.insert("vite_react_refresh", "");

                let vite_main = match main_integrity {
                    Some(main_integrity) => {
                        html! {
                            script type="module" src=(main_path) integrity=(main_integrity) {}
                        }.into_string()
                    },
                    None => {
                        html! {
                            script type="module" src=(main_path) {}
                        }.into_string()
                    }
                };

                context.insert(
                    "vite_main",
                    &vite_main,
                );

                let app_element = html! {
                    div #app data-page=(props) {}
                }
                .into_string();
                context.insert("application", &app_element);

                match &self.layout_template {
                    Some(layout_template) => {
                        match template_engine.render(layout_template, &context) {
                            Ok(output) => output,
                            Err(err) => {
                                eprintln!("Failed to render template {err}");
                                "".to_string()
                            }
                        }
                    },
                    None => "".to_string()
                }
            } else {
                let css = self.css.clone().unwrap_or("".to_string());
                html! {
                    html lang=(self.lang) {
                        head {
                            title { (self.title) }
                            meta charset="utf-8";
                            meta name="viewport" content="width=device-width, initial-scale=1.0";
                            @if let Some(integrity) = main_integrity {
                                script type="module" src=(main_path) integrity=(integrity) {}
                            } else {
                                script type="module" src=(main_path) {}
                            }
                            (PreEscaped(css))
                        }
                        body {
                            div #app data-page=(props) {}
                        }
                    }
                }
                .into_string()
            }

        });
        InertiaConfig::new(Some(self.version), layout)
    }
}

#[derive(Debug)]
pub enum ViteError {
    ManifestMissing(std::io::Error),
    EntryMissing(&'static str),
}

impl std::fmt::Display for ViteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestMissing(_) => write!(f, "couldn't open manifest file"),
            Self::EntryMissing(entry) => write!(f, "manifest missing entry for {}", entry),
        }
    }
}

impl std::error::Error for ViteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ManifestMissing(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct ManifestEntry {
    file: String,
    integrity: Option<String>,
    css: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_development_default() {
        let development = Development::default();

        assert_eq!(development.port, 5173);
        assert_eq!(development.main, "src/main.ts");
        assert_eq!(development.lang, "en");
        assert_eq!(development.title, "Vite");
        assert_eq!(development.react, false);
    }

    #[test]
    fn test_development_builder_methods() {
        let development = Development::default()
            .port(8080)
            .main("src/deep/index.ts")
            .lang("id")
            .title("Untitled Axum Inertia App")
            .react();

        assert_eq!(development.port, 8080);
        assert_eq!(development.main, "src/deep/index.ts");
        assert_eq!(development.lang, "id");
        assert_eq!(development.title, "Untitled Axum Inertia App");
        assert_eq!(development.react, true);
    }

    #[test]
    fn test_development_into_config() {
        let main_script = "src/index.ts";
        let development = Development::default()
            .port(8080)
            .main(main_script)
            .lang("lang-id")
            .title("app-title-here")
            .react();

        let config = development.into_config();

        assert_eq!(config.version(), None);

        let config_layout = config.layout();
        let binding = config_layout(r#"{"someprops": "somevalues"}"#.to_string());
        let rendered_layout = binding.as_str();

        assert!(rendered_layout.contains(r#"<html lang="lang-id">"#));
        assert!(rendered_layout.contains(r#"<title>app-title-here</title>"#));
        assert!(rendered_layout.contains(r#"{&quot;someprops&quot;: &quot;somevalues&quot;}"#));
        assert!(rendered_layout.contains(r#"http://localhost:8080/@vite/client"#));
        assert!(
            rendered_layout.contains(r#"window.__vite_plugin_react_preamble_installed__ = true"#)
        );
    }

    #[test]
    fn test_production_new_entry_missing() {
        let manifest_content = r#"{"main.js": {}}"#;
        let result = Production::new_from_string(manifest_content, "nonexistent.js");

        assert!(matches!(result, Err(_)));
    }

    #[test]
    fn test_production_new() {
        let manifest_content =
            r#"{"main.js": {"file": "main.hash-id-here.js", "css": ["style.css"]}}"#;
        let production_res = Production::new_from_string(manifest_content, "main.js");

        assert!(production_res.is_ok());

        let production = production_res.unwrap();
        let content_hash = encode(Sha1::digest(manifest_content.as_bytes()));

        assert_eq!(production.main.css, Some(vec!(String::from("style.css"))));
        assert_eq!(production.title, "Vite");
        assert_eq!(production.main.file, "main.hash-id-here.js");
        assert_eq!(production.main.integrity, None);
        assert_eq!(production.lang, "en");
        assert_eq!(production.version, content_hash);
    }

    #[test]
    fn test_production_builder_methods() {
        let manifest_content =
            r#"{"main.js": {"file": "main.hash-id-here.js", "css": ["style.css"]}}"#;
        let production = Production::new_from_string(manifest_content, "main.js")
            .unwrap()
            .lang("fr")
            .title("Untitled Axum Inertia App");

        assert_eq!(production.lang, "fr");
        assert_eq!(production.title, "Untitled Axum Inertia App");
    }

    #[test]
    fn test_production_into_config() {
        let manifest_content =
            r#"{"main.js": {"file": "main.hash-id-here.js", "css": ["style.css"]}}"#;
        let production = Production::new_from_string(manifest_content, "main.js")
            .unwrap()
            .lang("jv")
            .title("Untitled Axum Inertia App");

        let config = production.into_config();
        let config_layout = config.layout();
        let binding = config_layout(r#"{"someprops": "somevalues"}"#.to_string());
        let rendered_layout = binding.as_str();

        assert!(rendered_layout
            .contains(r#"<script type="module" src="/main.hash-id-here.js"></script>"#));
        assert!(rendered_layout.contains(r#"<link rel="stylesheet" href="/style.css"/>"#));
        assert!(rendered_layout.contains(r#"<html lang="jv">"#));
        assert!(rendered_layout.contains(r#"<title>Untitled Axum Inertia App</title>"#));
        assert!(rendered_layout.contains(r#"{&quot;someprops&quot;: &quot;somevalues&quot;}"#));
    }

    #[test]
    fn test_production_into_config_with_integrity() {
        let manifest_content = r#"{"main.js": {"file": "main.hash-id-here.js", "integrity": "sha000-shaHashHere1234", "css": ["style.css"]}}"#;
        let production = Production::new_from_string(manifest_content, "main.js")
            .unwrap()
            .lang("jv")
            .title("Untitled Axum Inertia App");

        let config = production.into_config();
        let config_layout = config.layout();
        let binding = config_layout(r#"{"someprops": "somevalues"}"#.to_string());
        let rendered_layout = binding.as_str();

        assert!(rendered_layout.contains(r#"<script type="module" src="/main.hash-id-here.js" integrity="sha000-shaHashHere1234"></script>"#));
        assert!(rendered_layout.contains(r#"<link rel="stylesheet" href="/style.css"/>"#));
        assert!(rendered_layout.contains(r#"<html lang="jv">"#));
        assert!(rendered_layout.contains(r#"<title>Untitled Axum Inertia App</title>"#));
        assert!(rendered_layout.contains(r#"{&quot;someprops&quot;: &quot;somevalues&quot;}"#));
    }
}
