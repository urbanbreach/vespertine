//! Release feed bodies: dynamic and static manifests and GitHub releases.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, SecondsFormat};
use semver::Version;
use serde::Deserialize;
use serde_json::{Map, Value};
use vespertine_http_client::Url;

use crate::error::{Error, Result};
use crate::platform::{Os, Platform};
use crate::version::parse_version;

/// The package formats a feed can advertise. Only [`UpdateFormat::AppImage`] is installable
/// on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateFormat {
    Nsis,
    Wix,
    App,
    AppImage,
}

impl UpdateFormat {
    /// Case-insensitive: `nsis`, `wix`, `app` or `appimage`.
    pub fn parse(text: &str) -> Result<Self> {
        match text.to_ascii_lowercase().as_str() {
            "nsis" => Ok(Self::Nsis),
            "wix" => Ok(Self::Wix),
            "app" => Ok(Self::App),
            "appimage" => Ok(Self::AppImage),
            _ => Err(Error::UnknownFormat(text.to_string())),
        }
    }

    /// The lowercase name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nsis => "nsis",
            Self::Wix => "wix",
            Self::App => "app",
            Self::AppImage => "appimage",
        }
    }
}

impl fmt::Display for UpdateFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where one platform's package and its signature are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Package {
    pub url: Url,
    /// The base64 text of the Minisign signature file; empty for a GitHub release that was
    /// not newer (the signature is then neither downloaded nor used).
    pub signature: String,
    pub format: UpdateFormat,
}

#[derive(Debug)]
pub(crate) enum Packages {
    /// A dynamic manifest or a resolved GitHub release: one package.
    Single(Package),
    /// A static manifest: packages by `<os>-<arch>` key.
    Platforms(BTreeMap<String, Package>),
}

/// A release as described by a manifest.
#[derive(Debug)]
pub(crate) struct Manifest {
    pub version: Version,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub packages: Packages,
}

/// A GitHub release whose package asset was selected but whose signature is not fetched yet.
#[derive(Debug)]
pub(crate) struct GithubRelease {
    pub version: Version,
    pub notes: Option<String>,
    pub date: Option<String>,
    pub package_url: Url,
    pub signature_url: Url,
}

#[derive(Debug)]
pub(crate) enum Feed {
    Manifest(Manifest),
    GitHub(GithubRelease),
}

/// Interprets a feed body: a manifest if it has `version` and either `url` or `platforms`,
/// else a GitHub release if it has `tag_name` and `assets`, else a manifest (which then
/// fails as malformed unless it is a manifest using the `name` alias).
pub(crate) fn parse_feed(body: &[u8], platform: &Platform) -> Result<Feed> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| Error::MalformedFeed(format!("not valid JSON: {error}")))?;
    let Value::Object(object) = value else {
        return Err(Error::MalformedFeed("expected a JSON object".to_string()));
    };
    let is_manifest = object.contains_key("version")
        && (object.contains_key("url") || object.contains_key("platforms"));
    if !is_manifest && object.contains_key("tag_name") && object.contains_key("assets") {
        return parse_github_release(&object, platform).map(Feed::GitHub);
    }
    parse_manifest(&object).map(Feed::Manifest)
}

fn optional_string<'a>(object: &'a Map<String, Value>, name: &str) -> Result<Option<&'a str>> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(Error::MalformedFeed(format!("`{name}` must be a string"))),
    }
}

fn required_string<'a>(object: &'a Map<String, Value>, name: &'static str) -> Result<&'a str> {
    optional_string(object, name)?.ok_or(Error::MissingField(name))
}

fn release_version(text: &str) -> Result<Version> {
    parse_version(text).map_err(|source| Error::InvalidReleaseVersion {
        version: text.to_string(),
        source,
    })
}

/// Normalizes an RFC 3339 timestamp; `None` when it is not one.
fn normalize_date(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn parse_manifest(object: &Map<String, Value>) -> Result<Manifest> {
    // `name` is an alias of `version`; `version` wins when both are present.
    let version = match optional_string(object, "version")? {
        Some(version) => version,
        None => required_string(object, "name").map_err(|error| match error {
            Error::MissingField(_) => Error::MissingField("version"),
            other => other,
        })?,
    };
    let version = release_version(version)?;
    let notes = optional_string(object, "notes")?.map(str::to_string);
    let date = optional_string(object, "pub_date")?
        .map(|text| normalize_date(text).ok_or_else(|| Error::InvalidDate(text.to_string())))
        .transpose()?;
    let packages = if !object.contains_key("url") && object.contains_key("platforms") {
        let Some(Value::Object(platforms)) = object.get("platforms") else {
            return Err(Error::MalformedFeed(
                "`platforms` must be an object".to_string(),
            ));
        };
        let mut packages = BTreeMap::new();
        for (key, value) in platforms {
            let Value::Object(entry) = value else {
                return Err(Error::MalformedFeed(format!(
                    "platform `{key}` must be an object"
                )));
            };
            packages.insert(key.clone(), parse_package(entry)?);
        }
        Packages::Platforms(packages)
    } else {
        Packages::Single(parse_package(object)?)
    };
    Ok(Manifest {
        version,
        notes,
        date,
        packages,
    })
}

fn parse_package(object: &Map<String, Value>) -> Result<Package> {
    let url = required_string(object, "url")?;
    let signature = required_string(object, "signature")?;
    let format = required_string(object, "format")?;
    Ok(Package {
        url: Url::parse(url).map_err(|error| {
            Error::MalformedFeed(format!("invalid package URL `{url}`: {error}"))
        })?,
        signature: signature.to_string(),
        format: UpdateFormat::parse(format)?,
    })
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn parse_github_release(object: &Map<String, Value>, platform: &Platform) -> Result<GithubRelease> {
    let version = release_version(required_string(object, "tag_name")?)?;
    let notes = optional_string(object, "body")?.map(str::to_string);
    // A publication date that is not RFC 3339 is dropped, not an error.
    let date = optional_string(object, "published_at")?.and_then(normalize_date);
    let assets: Vec<Asset> = serde_json::from_value(object["assets"].clone())
        .map_err(|error| Error::MalformedFeed(format!("invalid `assets`: {error}")))?;
    if platform.os() != Os::Linux {
        return Err(Error::UnsupportedPlatform(format!(
            "GitHub release feeds are only supported on linux, not {}",
            platform.target()
        )));
    }
    let suffix = format!("_{}.AppImage", platform.arch());
    let package = assets
        .iter()
        .find(|asset| !asset.name.ends_with(".sig") && asset.name.ends_with(&suffix))
        .ok_or_else(|| Error::AssetNotFound {
            target: platform.target().to_string(),
            arch: platform.arch().to_string(),
        })?;
    let signature_name = format!("{}.sig", package.name);
    let signature = assets
        .iter()
        .find(|asset| asset.name == signature_name)
        .ok_or(Error::SignatureAssetMissing(signature_name))?;
    let asset_url = |asset: &Asset| {
        Url::parse(&asset.browser_download_url).map_err(|error| {
            Error::MalformedFeed(format!(
                "invalid download URL `{}` of asset `{}`: {error}",
                asset.browser_download_url, asset.name
            ))
        })
    };
    Ok(GithubRelease {
        version,
        notes,
        date,
        package_url: asset_url(package)?,
        signature_url: asset_url(signature)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> Platform {
        Platform::from_parts("linux", "x86_64").unwrap()
    }

    fn manifest(body: &str) -> Result<Manifest> {
        match parse_feed(body.as_bytes(), &linux())? {
            Feed::Manifest(manifest) => Ok(manifest),
            Feed::GitHub(_) => panic!("expected a manifest"),
        }
    }

    #[test]
    fn a_dynamic_manifest_carries_every_field() {
        let manifest = manifest(
            r#"{"version":"v2.0.0","url":"https://e.test/app.AppImage","signature":"c2ln","format":"AppImage","notes":"Fixes","pub_date":"2024-05-06T07:08:09Z"}"#,
        )
        .unwrap();
        assert_eq!(manifest.version, Version::new(2, 0, 0));
        assert_eq!(manifest.notes.as_deref(), Some("Fixes"));
        assert_eq!(manifest.date.as_deref(), Some("2024-05-06T07:08:09Z"));
        let Packages::Single(package) = manifest.packages else {
            panic!("expected a single package")
        };
        assert_eq!(package.url.as_str(), "https://e.test/app.AppImage");
        assert_eq!(package.signature, "c2ln");
        assert_eq!(package.format, UpdateFormat::AppImage);
    }

    #[test]
    fn the_name_alias_and_optional_fields() {
        let manifest =
            manifest(r#"{"name":"1.2.3","url":"https://e.test/a","signature":"s","format":"APP"}"#)
                .unwrap();
        assert_eq!(manifest.version, Version::new(1, 2, 3));
        assert_eq!(manifest.notes, None);
        assert_eq!(manifest.date, None);
        let manifest = self::manifest(
            r#"{"version":"1.0.0","name":"9.9.9","url":"https://e.test/a","signature":"s","format":"wix"}"#,
        )
        .unwrap();
        assert_eq!(manifest.version, Version::new(1, 0, 0));
    }

    #[test]
    fn formats_are_case_insensitive_and_closed() {
        for (text, format) in [
            ("nsis", UpdateFormat::Nsis),
            ("NSIS", UpdateFormat::Nsis),
            ("Wix", UpdateFormat::Wix),
            ("app", UpdateFormat::App),
            ("AppImage", UpdateFormat::AppImage),
        ] {
            assert_eq!(UpdateFormat::parse(text).unwrap(), format);
        }
        assert!(matches!(
            UpdateFormat::parse("deb"),
            Err(Error::UnknownFormat(_))
        ));
    }

    #[test]
    fn dynamic_manifests_name_the_missing_field() {
        for (missing, body) in [
            (
                "url",
                r#"{"version":"1.0.0","url":null,"signature":"s","format":"app"}"#,
            ),
            (
                "signature",
                r#"{"version":"1.0.0","url":"https://e.test/a","format":"app"}"#,
            ),
            (
                "format",
                r#"{"version":"1.0.0","url":"https://e.test/a","signature":"s"}"#,
            ),
        ] {
            let error = manifest(body).unwrap_err();
            assert!(
                matches!(error, Error::MissingField(name) if name == missing),
                "{missing}: {error}"
            );
            assert!(error.to_string().contains(missing));
        }
    }

    #[test]
    fn an_invalid_manifest_date_is_an_error() {
        let error = manifest(
            r#"{"version":"1.0.0","url":"https://e.test/a","signature":"s","format":"app","pub_date":"yesterday"}"#,
        )
        .unwrap_err();
        assert!(matches!(error, Error::InvalidDate(_)), "{error}");
    }

    #[test]
    fn static_manifests_key_packages_by_platform() {
        let manifest = manifest(
            r#"{"version":"3.0.0","platforms":{"linux-x86_64":{"url":"https://e.test/a","signature":"s","format":"appimage"},"windows-x86_64":{"url":"https://e.test/b","signature":"t","format":"nsis"}}}"#,
        )
        .unwrap();
        let Packages::Platforms(platforms) = manifest.packages else {
            panic!("expected platforms")
        };
        assert_eq!(platforms.len(), 2);
        assert_eq!(platforms["windows-x86_64"].format, UpdateFormat::Nsis);
    }

    #[test]
    fn malformed_bodies_fail_as_manifests() {
        for body in [
            "not json",
            "[]",
            "{}",
            r#"{"url":"https://e.test/a"}"#,
            r#"{"tag_name":"v1.0.0"}"#,
        ] {
            assert!(
                matches!(
                    parse_feed(body.as_bytes(), &linux()),
                    Err(Error::MalformedFeed(_) | Error::MissingField(_))
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn a_github_release_is_recognized_by_tag_name_and_assets() {
        let feed = parse_feed(
            br#"{"tag_name":"v1.5.0","name":"Release","url":"https://api.github.com/x","body":"Notes","published_at":"not a date","assets":[
              {"name":"notes.txt","browser_download_url":"https://e.test/notes"},
              {"name":"app_aarch64.AppImage","browser_download_url":"https://e.test/arm"},
              {"name":"app_x86_64.AppImage","browser_download_url":"https://e.test/pkg"},
              {"name":"app_x86_64.AppImage.sig","browser_download_url":"https://e.test/sig"}]}"#,
            &linux(),
        )
        .unwrap();
        let Feed::GitHub(release) = feed else {
            panic!("expected a GitHub release")
        };
        assert_eq!(release.version, Version::new(1, 5, 0));
        assert_eq!(release.notes.as_deref(), Some("Notes"));
        assert_eq!(release.date, None);
        assert_eq!(release.package_url.as_str(), "https://e.test/pkg");
        assert_eq!(release.signature_url.as_str(), "https://e.test/sig");
    }
}
