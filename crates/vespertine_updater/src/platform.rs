use std::fmt;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use semver::Version;
use vespertine_http_client::Url;

use crate::error::{Error, Result};

/// Control characters and `+` are escaped when a version is substituted into a URL.
const VERSION_ENCODE_SET: &AsciiSet = &CONTROLS.add(b'+');

const CURRENT_VERSION_SLOT: &str = "{{current_version}}";
const CURRENT_VERSION_SLOT_ENCODED: &str = "%7B%7Bcurrent_version%7D%7D";
const TARGET_SLOT: &str = "{{target}}";
const TARGET_SLOT_ENCODED: &str = "%7B%7Btarget%7D%7D";
const ARCH_SLOT: &str = "{{arch}}";
const ARCH_SLOT_ENCODED: &str = "%7B%7Barch%7D%7D";

/// The operating system a package is installed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    /// The lowercase operating-system name used for the `{{target}}` slot and platform keys.
    pub fn name(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Windows => "windows",
        }
    }
}

/// The running platform: operating system and machine architecture name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    os: Os,
    arch: &'static str,
}

impl Platform {
    /// The platform this process runs on.
    pub fn detect() -> Result<Self> {
        Self::from_parts(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Maps Rust's operating-system and architecture names (`std::env::consts`) to the
    /// updater's: `x86`→`i686`, `x86_64`→`x86_64`, `arm`→`armv7`, `aarch64`→`aarch64`,
    /// `riscv64`→`riscv64`. Anything else is an unsupported platform.
    pub fn from_parts(os: &str, arch: &str) -> Result<Self> {
        let os = match os {
            "linux" => Os::Linux,
            "macos" => Os::MacOs,
            "windows" => Os::Windows,
            other => {
                return Err(Error::UnsupportedPlatform(format!(
                    "operating system `{other}`"
                )));
            }
        };
        let arch = match arch {
            "x86" => "i686",
            "x86_64" => "x86_64",
            "arm" => "armv7",
            "aarch64" => "aarch64",
            "riscv64" => "riscv64",
            other => {
                return Err(Error::UnsupportedPlatform(format!(
                    "architecture `{other}`"
                )));
            }
        };
        Ok(Self { os, arch })
    }

    pub fn os(&self) -> Os {
        self.os
    }

    /// The lowercase operating-system name (`linux`).
    pub fn target(&self) -> &'static str {
        self.os.name()
    }

    /// The machine architecture name (`x86_64`, `aarch64`, ...).
    pub fn arch(&self) -> &'static str {
        self.arch
    }

    /// The platform key of static manifests: `<os>-<arch>`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.target(), self.arch)
    }

    /// Substitutes the `current_version`, `target` and `arch` slots (literal and
    /// percent-encoded forms) of `endpoint`, re-parses the result and translates GitHub
    /// repository URLs to their latest-release API URL.
    pub fn expand_endpoint(&self, endpoint: &Url, current_version: &Version) -> Result<Url> {
        let version =
            utf8_percent_encode(&current_version.to_string(), VERSION_ENCODE_SET).to_string();
        let substituted = endpoint
            .as_str()
            .replace(CURRENT_VERSION_SLOT_ENCODED, &version)
            .replace(CURRENT_VERSION_SLOT, &version)
            .replace(TARGET_SLOT_ENCODED, self.target())
            .replace(TARGET_SLOT, self.target())
            .replace(ARCH_SLOT_ENCODED, self.arch)
            .replace(ARCH_SLOT, self.arch);
        let url = Url::parse(&substituted).map_err(|source| Error::InvalidUrl {
            url: substituted,
            source,
        })?;
        Ok(github_release_api_url(&url).unwrap_or(url))
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// `https://api.github.com/repos/{owner}/{repo}/releases/latest` for a github.com
/// repository URL (at least two non-empty path segments, the first not `repos`).
fn github_release_api_url(url: &Url) -> Option<Url> {
    if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return None;
    }
    let mut segments = url.path_segments()?.filter(|segment| !segment.is_empty());
    let owner = segments.next()?;
    let repo = segments.next()?;
    if owner == "repos" {
        return None;
    }
    Url::parse(&format!(
        "https://api.github.com/repos/{owner}/{repo}/releases/latest"
    ))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> Platform {
        Platform::from_parts("linux", "x86_64").unwrap()
    }

    fn expand(endpoint: &str, version: &str) -> String {
        platform()
            .expand_endpoint(
                &Url::parse(endpoint).unwrap(),
                &crate::version::parse_version(version).unwrap(),
            )
            .unwrap()
            .to_string()
    }

    #[test]
    fn architecture_names_are_mapped_and_unknown_ones_rejected() {
        for (rust, name) in [
            ("x86", "i686"),
            ("x86_64", "x86_64"),
            ("arm", "armv7"),
            ("aarch64", "aarch64"),
            ("riscv64", "riscv64"),
        ] {
            let platform = Platform::from_parts("linux", rust).unwrap();
            assert_eq!(platform.arch(), name);
            assert_eq!(platform.key(), format!("linux-{name}"));
        }
        for arch in ["powerpc64", "s390x", "mips", ""] {
            assert!(matches!(
                Platform::from_parts("linux", arch),
                Err(Error::UnsupportedPlatform(_))
            ));
        }
        for os in ["freebsd", "android", "ios", ""] {
            assert!(matches!(
                Platform::from_parts(os, "x86_64"),
                Err(Error::UnsupportedPlatform(_))
            ));
        }
        assert_eq!(
            Platform::from_parts("linux", "x86_64").unwrap().target(),
            "linux"
        );
    }

    #[test]
    fn slots_are_substituted_in_literal_and_percent_encoded_form() {
        // The URL parser percent-encodes braces in the path, not in the query.
        let url = Url::parse("https://e.test/{{target}}/{{arch}}/{{current_version}}?v={{current_version}}&t={{target}}&a={{arch}}").unwrap();
        assert!(url.as_str().contains("%7B%7Btarget%7D%7D"));
        assert!(url.as_str().contains("v={{current_version}}"));
        assert_eq!(
            expand(url.as_str(), "1.2.3"),
            "https://e.test/linux/x86_64/1.2.3?v=1.2.3&t=linux&a=x86_64"
        );
    }

    #[test]
    fn version_substitution_escapes_plus_and_strips_leading_v() {
        assert_eq!(
            expand("https://e.test/{{current_version}}", "v1.0.0+build.5"),
            "https://e.test/1.0.0%2Bbuild.5"
        );
        assert_eq!(
            expand("https://e.test/?v={{current_version}}", "1.0.0-rc.1+x"),
            "https://e.test/?v=1.0.0-rc.1%2Bx"
        );
    }

    #[test]
    fn github_repository_urls_become_the_latest_release_api_url() {
        let api = "https://api.github.com/repos/owner/repo/releases/latest";
        assert_eq!(expand("https://github.com/owner/repo", "1.0.0"), api);
        assert_eq!(expand("https://www.github.com/owner/repo/", "1.0.0"), api);
        assert_eq!(
            expand("https://github.com/owner/repo/releases/latest?x=1", "1.0.0"),
            api
        );
        assert_eq!(expand("https://github.com//owner//repo", "1.0.0"), api);
    }

    #[test]
    fn github_urls_that_are_not_repositories_are_left_alone() {
        for url in [
            "https://github.com/owner",
            "https://github.com/",
            "https://github.com/repos/owner/repo",
            "https://api.github.com/repos/owner/repo/releases/latest",
            "https://example.com/owner/repo",
            "https://gist.github.com/owner/repo",
        ] {
            assert_eq!(expand(url, "1.0.0"), Url::parse(url).unwrap().to_string());
        }
    }
}
