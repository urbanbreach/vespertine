use std::cmp::Ordering;

use semver::Version;

/// Parses a semantic version after stripping any number of leading `v` characters.
pub fn parse_version(text: &str) -> Result<Version, semver::Error> {
    Version::parse(text.trim_start_matches('v'))
}

/// Semantic-version precedence: build metadata does not take part in the comparison
/// (so `1.0.0+a` and `1.0.0+b` are equal), prereleases order per the standard.
pub fn precedence(a: &Version, b: &Version) -> Ordering {
    (a.major, a.minor, a.patch)
        .cmp(&(b.major, b.minor, b.patch))
        .then_with(|| a.pre.cmp(&b.pre))
}

/// Whether `candidate` is strictly newer than `current`.
pub fn is_newer(candidate: &Version, current: &Version) -> bool {
    precedence(candidate, current) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        parse_version(text).unwrap()
    }

    #[test]
    fn leading_v_characters_are_stripped() {
        assert_eq!(v("v1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("vvv1.2.3"), Version::new(1, 2, 3));
        assert!(parse_version("1.2").is_err());
        assert!(parse_version("V1.2.3").is_err());
        assert!(parse_version("").is_err());
    }

    #[test]
    fn newer_follows_semantic_version_precedence() {
        assert!(is_newer(&v("v1.0.1"), &v("1.0.0")));
        assert!(is_newer(&v("1.10.0"), &v("1.9.9")));
        assert!(is_newer(&v("1.0.0"), &v("1.0.0-rc.1")));
        assert!(is_newer(&v("1.0.0-rc.2"), &v("1.0.0-rc.1")));
        assert!(is_newer(&v("1.0.0-rc.10"), &v("1.0.0-rc.9")));
        assert!(is_newer(&v("1.0.0-alpha.1"), &v("1.0.0-alpha")));
        assert!(is_newer(&v("1.0.0-beta"), &v("1.0.0-alpha.99")));
        assert!(is_newer(&v("1.0.0-1"), &v("1.0.0-0")));
        assert!(is_newer(&v("1.0.0-alpha"), &v("1.0.0-99")));
        assert!(!is_newer(&v("1.0.0"), &v("1.0.0")));
        assert!(!is_newer(&v("1.0.0-rc.1"), &v("1.0.0")));
        assert!(!is_newer(&v("0.9.9"), &v("1.0.0")));
        assert!(!is_newer(&v("1.0.0+b"), &v("1.0.0+a")));
    }
}
