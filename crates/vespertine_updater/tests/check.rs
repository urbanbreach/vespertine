//! Feed checking (APP-004 A1, A2, A5): manifests, GitHub releases, endpoint iteration, input
//! validation and request behavior, against local HTTP servers.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use support::*;
use vespertine_updater::{
    CheckOptions, Error, Header, InstallMode, Platform, Update, UpdateFormat, Updater,
};

fn options(endpoints: &[String]) -> CheckOptions {
    CheckOptions {
        endpoints: endpoints.to_vec(),
        pubkey: key().public_key(),
        ..CheckOptions::default()
    }
}

fn check(version: &str, options: &CheckOptions) -> Result<Option<Update>, Error> {
    block_on(check_only_updater().check(version, options))
}

fn feed_server(body: impl Into<String>) -> Server {
    let body = body.into();
    Server::start(move |_, _| Reply::ok(body.clone()))
}

// ---------------------------------------------------------------------------------------
// A1: manifests, versions, endpoint iteration, slots

#[test]
fn a1_a_dynamic_manifest_with_a_newer_version_yields_every_update_field() {
    let server = Server::start(|base, _| {
        Reply::ok(dynamic_manifest(
            "v2.1.0",
            &format!("{base}/package"),
            "c2ln",
            "AppImage",
        ))
    });
    let update = check("v1.0.0", &options(&[server.url("/feed")]))
        .unwrap()
        .expect("an update");
    assert_eq!(update.current_version(), "1.0.0");
    assert_eq!(update.version(), "2.1.0");
    assert_eq!(update.notes(), Some("Release notes"));
    assert_eq!(update.date(), Some("2025-01-02T03:04:05Z"));
    assert_eq!(update.download_url(), server.url("/package"));
    assert_eq!(update.format(), UpdateFormat::AppImage);
    assert_eq!(update.format().name(), "appimage");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/feed");
}

#[test]
fn a1_a_static_manifest_resolves_the_running_platform_key() {
    let server = Server::start(|base, _| {
        Reply::ok(
            serde_json::json!({
                "version": "2.0.0",
                "platforms": {
                    "linux-aarch64": {"url": format!("{base}/arm"), "signature": "a", "format": "appimage"},
                    "linux-x86_64": {"url": format!("{base}/intel"), "signature": "b", "format": "appimage"},
                    "windows-x86_64": {"url": format!("{base}/win"), "signature": "c", "format": "NSIS"},
                },
            })
            .to_string(),
        )
    });
    let options = options(&[server.url("/feed")]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    assert_eq!(update.download_url(), server.url("/intel"));

    let arm = check_only_updater().with_platform(Platform::from_parts("linux", "aarch64").unwrap());
    let update = block_on(arm.check("1.0.0", &options)).unwrap().unwrap();
    assert_eq!(update.download_url(), server.url("/arm"));
}

#[test]
fn a1_a_static_manifest_without_the_platform_key_fails_only_for_a_newer_release() {
    let server = Server::start(|base, _| {
        Reply::ok(
            serde_json::json!({
                "version": "2.0.0",
                "platforms": {"windows-x86_64": {"url": format!("{base}/win"), "signature": "c", "format": "nsis"}},
            })
            .to_string(),
        )
    });
    let options = options(&[server.url("/feed")]);
    let error = check("1.0.0", &options).unwrap_err();
    assert!(
        matches!(&error, Error::TargetNotFound(key) if key == "linux-x86_64"),
        "{error}"
    );
    assert!(check("2.0.0", &options).unwrap().is_none());
    assert!(check("3.0.0", &options).unwrap().is_none());
}

#[test]
fn a1_the_name_alias_is_accepted_and_missing_dynamic_fields_are_named() {
    let server = feed_server(
        r#"{"name":"v3.0.0","url":"http://127.0.0.1:1/p","signature":"s","format":"app"}"#,
    );
    let update = check("1.0.0", &options(&[server.url("/feed")]))
        .unwrap()
        .unwrap();
    assert_eq!(update.version(), "3.0.0");
    assert_eq!(update.notes(), None);
    assert_eq!(update.date(), None);
    assert_eq!(update.format(), UpdateFormat::App);

    for (field, body) in [
        (
            "url",
            r#"{"version":"3.0.0","signature":"s","format":"app"}"#,
        ),
        (
            "signature",
            r#"{"version":"3.0.0","url":"http://127.0.0.1:1/p","format":"app"}"#,
        ),
        (
            "format",
            r#"{"version":"3.0.0","url":"http://127.0.0.1:1/p","signature":"s"}"#,
        ),
    ] {
        let server = feed_server(body);
        let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
        assert!(
            matches!(&error, Error::MissingField(name) if *name == field),
            "{field}: {error}"
        );
        assert!(error.to_string().contains(field), "{error}");
    }
}

#[test]
fn a1_the_format_is_case_insensitive_and_must_be_known() {
    for (format, expected) in [
        ("NSIS", UpdateFormat::Nsis),
        ("Wix", UpdateFormat::Wix),
        ("APP", UpdateFormat::App),
        ("aPpImAgE", UpdateFormat::AppImage),
    ] {
        let server = feed_server(dynamic_manifest(
            "2.0.0",
            "http://127.0.0.1:1/p",
            "s",
            format,
        ));
        let update = check("1.0.0", &options(&[server.url("/feed")]))
            .unwrap()
            .unwrap();
        assert_eq!(update.format(), expected, "{format}");
    }
    let server = feed_server(dynamic_manifest(
        "2.0.0",
        "http://127.0.0.1:1/p",
        "s",
        "deb",
    ));
    let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
    assert!(matches!(error, Error::UnknownFormat(_)), "{error}");
}

#[test]
fn a1_an_invalid_manifest_date_is_an_error_and_a_valid_offset_date_is_kept() {
    let body = |date: &str| {
        serde_json::json!({
            "version": "2.0.0", "url": "http://127.0.0.1:1/p", "signature": "s",
            "format": "app", "pub_date": date,
        })
        .to_string()
    };
    let server = feed_server(body("2025-13-45"));
    let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
    assert!(matches!(error, Error::InvalidDate(_)), "{error}");

    let server = feed_server(body("2025-06-07T08:09:10+02:00"));
    let update = check("1.0.0", &options(&[server.url("/feed")]))
        .unwrap()
        .unwrap();
    assert_eq!(update.date(), Some("2025-06-07T08:09:10+02:00"));
}

#[test]
fn a1_leading_v_characters_are_stripped_from_both_versions() {
    let server = feed_server(dynamic_manifest(
        "vv1.0.1",
        "http://127.0.0.1:1/p",
        "s",
        "app",
    ));
    let update = check("vvv1.0.0", &options(&[server.url("/feed")]))
        .unwrap()
        .unwrap();
    assert_eq!(update.current_version(), "1.0.0");
    assert_eq!(update.version(), "1.0.1");
}

#[test]
fn a1_equal_older_and_prerelease_releases_return_null() {
    for (current, release) in [
        ("1.0.0", "1.0.0"),
        ("v1.0.0", "1.0.0"),
        ("1.0.0", "0.9.9"),
        ("1.0.0", "1.0.0-rc.1"),
        ("1.0.0", "1.0.0+build.7"),
        ("1.0.0-rc.2", "1.0.0-rc.1"),
    ] {
        let server = feed_server(dynamic_manifest(
            release,
            "http://127.0.0.1:1/p",
            "s",
            "app",
        ));
        let found = check(current, &options(&[server.url("/feed")])).unwrap();
        assert!(found.is_none(), "{current} vs {release}");
    }
    let server = feed_server(dynamic_manifest(
        "1.0.0",
        "http://127.0.0.1:1/p",
        "s",
        "app",
    ));
    assert!(
        check("1.0.0-rc.1", &options(&[server.url("/feed")]))
            .unwrap()
            .is_some()
    );
}

#[test]
fn a1_a_204_response_ends_the_check_before_later_endpoints() {
    let server = Server::start(|base, request| match request.target.as_str() {
        "/none" => Reply::status(204),
        _ => Reply::ok(dynamic_manifest("9.0.0", &format!("{base}/p"), "s", "app")),
    });
    let found = check(
        "1.0.0",
        &options(&[server.url("/none"), server.url("/newer")]),
    )
    .unwrap();
    assert!(found.is_none());
    assert_eq!(server.targets(), ["/none"]);

    // Earlier failures are forgotten as well.
    let found = check("1.0.0", &options(&[closed_port_url(), server.url("/none")])).unwrap();
    assert!(found.is_none());
}

#[test]
fn a1_failed_endpoints_are_skipped_in_order_until_one_yields_a_release() {
    let server = Server::start(|base, request| match request.target.as_str() {
        "/error" => Reply::status(500),
        "/garbage" => Reply::ok("<html>not json</html>"),
        _ => Reply::ok(dynamic_manifest("2.0.0", &format!("{base}/p"), "s", "app")),
    });
    let endpoints = [
        closed_port_url(),
        server.url("/error"),
        server.url("/garbage"),
        server.url("/good"),
        server.url("/never"),
    ];
    let update = check("1.0.0", &options(&endpoints)).unwrap().unwrap();
    assert_eq!(update.version(), "2.0.0");
    assert_eq!(server.targets(), ["/error", "/garbage", "/good"]);
}

#[test]
fn a1_a_later_success_clears_earlier_errors() {
    let server = Server::start(|base, request| match request.target.as_str() {
        "/garbage" => Reply::ok("nope"),
        _ => Reply::ok(dynamic_manifest("1.0.0", &format!("{base}/p"), "s", "app")),
    });
    let found = check(
        "1.0.0",
        &options(&[server.url("/garbage"), server.url("/same")]),
    )
    .unwrap();
    assert!(found.is_none());
}

#[test]
fn a1_the_last_remembered_error_is_reported_when_every_endpoint_fails() {
    let server = Server::start(|_, request| match request.target.as_str() {
        "/garbage" => Reply::ok("not json"),
        "/unsigned" => {
            Reply::ok(r#"{"version":"2.0.0","url":"http://127.0.0.1:1/p","format":"app"}"#)
        }
        _ => Reply::status(404),
    });
    let error = check(
        "1.0.0",
        &options(&[server.url("/garbage"), server.url("/unsigned")]),
    )
    .unwrap_err();
    assert!(matches!(error, Error::MissingField("signature")), "{error}");

    // A non-success status does not replace a remembered error.
    let error = check(
        "1.0.0",
        &options(&[
            server.url("/unsigned"),
            server.url("/garbage"),
            server.url("/missing"),
        ]),
    )
    .unwrap_err();
    assert!(matches!(error, Error::MalformedFeed(_)), "{error}");

    // A later transport failure does.
    let error = check(
        "1.0.0",
        &options(&[server.url("/garbage"), closed_port_url()]),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Request { .. }), "{error}");
}

#[test]
fn a1_only_non_success_statuses_report_a_missing_release() {
    let server = Server::start(|_, request| match request.target.as_str() {
        "/a" => Reply::status(404),
        _ => Reply::status(500),
    });
    let error = check("1.0.0", &options(&[server.url("/a"), server.url("/b")])).unwrap_err();
    assert!(matches!(error, Error::ReleaseNotFound), "{error}");
    assert_eq!(error.to_string(), "could not fetch a valid release");
    assert_eq!(server.targets(), ["/a", "/b"]);
}

#[test]
fn a1_slots_are_substituted_before_each_request() {
    let server = Server::start(|_, _| Reply::status(204));
    let found = check(
        "v1.2.3+build.5",
        &options(&[server.url("/{{target}}/{{arch}}/{{current_version}}?v={{current_version}}&t={{target}}&a={{arch}}")]),
    )
    .unwrap();
    assert!(found.is_none());
    assert_eq!(
        server.targets(),
        ["/linux/x86_64/1.2.3%2Bbuild.5?v=1.2.3%2Bbuild.5&t=linux&a=x86_64"]
    );
}

#[test]
fn a1_github_repository_endpoints_are_requested_as_the_latest_release_api_url() {
    let client = RecordingClient::new(404);
    let updater = Updater::new(client.clone())
        .unwrap()
        .with_platform(linux_x86_64());
    let endpoints = [
        "https://github.com/owner/repo",
        "https://www.github.com/owner/repo/releases",
        "https://github.com/repos/owner/repo",
        "https://github.com/owner",
        "https://example.com/owner/repo",
    ]
    .map(String::from);
    let error = block_on(updater.check("1.0.0", &options(&endpoints))).unwrap_err();
    assert!(matches!(error, Error::ReleaseNotFound), "{error}");
    assert_eq!(
        client.uris(),
        [
            "https://api.github.com/repos/owner/repo/releases/latest",
            "https://api.github.com/repos/owner/repo/releases/latest",
            "https://github.com/repos/owner/repo",
            "https://github.com/owner",
            "https://example.com/owner/repo",
        ]
    );
}

// ---------------------------------------------------------------------------------------
// A2: GitHub releases

const SIGNATURE_TEXT: &str = "c2lnbmF0dXJl";

fn github_release(base: &str, tag: &str, assets: &[&str]) -> String {
    serde_json::json!({
        "url": "https://api.github.com/repos/owner/repo/releases/1",
        "name": "Release title",
        "tag_name": tag,
        "body": "Changelog",
        "published_at": "2025-03-04T05:06:07Z",
        "assets": assets.iter().map(|name| serde_json::json!({
            "name": name,
            "browser_download_url": format!("{base}/assets/{name}"),
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn github_server(tag: &'static str, assets: &'static [&'static str]) -> Server {
    Server::start(move |base, request| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, tag, assets)),
        target if target.ends_with(".sig") => Reply::ok(SIGNATURE_TEXT),
        _ => Reply::ok("package"),
    })
}

const ALL_ASSETS: &[&str] = &[
    "notes.txt",
    "app_aarch64.AppImage.sig",
    "app_aarch64.AppImage",
    "app_x86_64.AppImage.sig",
    "app_x86_64.AppImage",
    "other_x86_64.AppImage",
];

#[test]
fn a2_a_newer_release_selects_the_arch_asset_and_downloads_its_signature() {
    let server = github_server("v1.2.0", ALL_ASSETS);
    let mut options = options(&[server.url("/feed")]);
    options.headers = Some(vec![Header::new("x-token", "secret")]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    assert_eq!(update.version(), "1.2.0");
    assert_eq!(update.notes(), Some("Changelog"));
    assert_eq!(update.date(), Some("2025-03-04T05:06:07Z"));
    assert_eq!(
        update.download_url(),
        server.url("/assets/app_x86_64.AppImage")
    );
    assert_eq!(update.format(), UpdateFormat::AppImage);
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.target.as_str())
            .collect::<Vec<_>>(),
        ["/feed", "/assets/app_x86_64.AppImage.sig"]
    );
    for request in &requests {
        assert_eq!(
            request.header("x-token"),
            Some("secret"),
            "{}",
            request.target
        );
    }

    let arm = check_only_updater().with_platform(Platform::from_parts("linux", "aarch64").unwrap());
    let update = block_on(arm.check("1.0.0", &options)).unwrap().unwrap();
    assert_eq!(
        update.download_url(),
        server.url("/assets/app_aarch64.AppImage")
    );
}

#[test]
fn a2_the_signature_is_fetched_only_for_a_newer_release() {
    let server = github_server("v1.0.0", ALL_ASSETS);
    let found = check("v1.0.0", &options(&[server.url("/feed")])).unwrap();
    assert!(found.is_none());
    assert_eq!(server.targets(), ["/feed"]);
}

#[test]
fn a2_a_missing_sibling_signature_is_an_error_even_when_the_release_is_not_newer() {
    // `app_x86_64.AppImage.sig.txt` and a signature of another package are not siblings.
    const ASSETS: &[&str] = &[
        "app_x86_64.AppImage",
        "app_x86_64.AppImage.sig.txt",
        "other_x86_64.AppImage.sig",
    ];
    for tag in ["v0.5.0", "v1.0.0", "v2.0.0"] {
        let server = github_server(tag, ASSETS);
        let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
        assert!(
            matches!(&error, Error::SignatureAssetMissing(name) if name == "app_x86_64.AppImage.sig"),
            "{tag}: {error}"
        );
        assert_eq!(server.targets(), ["/feed"], "{tag}");
    }
}

#[test]
fn a2_a_failing_or_non_utf8_signature_fetch_is_an_error() {
    let missing = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, "v2.0.0", ALL_ASSETS)),
        _ => Reply::status(404),
    });
    let error = check("1.0.0", &options(&[missing.url("/feed")])).unwrap_err();
    assert!(
        matches!(&error, Error::SignatureStatus(status) if status.as_u16() == 404),
        "{error}"
    );

    let binary = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, "v2.0.0", ALL_ASSETS)),
        _ => Reply::ok(vec![0xff, 0xfe, 0x00, 0x80]),
    });
    let error = check("1.0.0", &options(&[binary.url("/feed")])).unwrap_err();
    assert!(matches!(error, Error::SignatureNotUtf8), "{error}");

    let unreachable = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, "v2.0.0", ALL_ASSETS)),
        _ => Reply::status(500),
    });
    let error = check("1.0.0", &options(&[unreachable.url("/feed")])).unwrap_err();
    assert!(matches!(error, Error::SignatureStatus(_)), "{error}");
}

#[test]
fn a2_no_matching_asset_is_an_error_naming_the_target_and_arch() {
    const ASSETS: &[&str] = &[
        "app_aarch64.AppImage",
        "app_aarch64.AppImage.sig",
        "app_x86_64.AppImage.sig",
        "app.AppImage",
    ];
    let server = github_server("v2.0.0", ASSETS);
    let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
    assert!(
        matches!(&error, Error::AssetNotFound { target, arch } if target == "linux" && arch == "x86_64"),
        "{error}"
    );
    assert!(error.to_string().contains("linux") && error.to_string().contains("x86_64"));
}

#[test]
fn a2_the_first_matching_asset_in_feed_order_wins_and_the_tag_is_normalized() {
    const ASSETS: &[&str] = &[
        "app_x86_64.AppImage.sig",
        "b_x86_64.AppImage",
        "a_x86_64.AppImage",
        "a_x86_64.AppImage.sig",
        "b_x86_64.AppImage.sig",
    ];
    let server = github_server("vv3.1.4", ASSETS);
    let update = check("3.1.3", &options(&[server.url("/feed")]))
        .unwrap()
        .unwrap();
    assert_eq!(update.version(), "3.1.4");
    assert_eq!(
        update.download_url(),
        server.url("/assets/b_x86_64.AppImage")
    );
}

#[test]
fn a2_a_tag_that_is_not_a_semantic_version_is_an_error() {
    let server = github_server("latest", ALL_ASSETS);
    let error = check("1.0.0", &options(&[server.url("/feed")])).unwrap_err();
    assert!(
        matches!(error, Error::InvalidReleaseVersion { .. }),
        "{error}"
    );
}

#[test]
fn a2_an_invalid_publication_date_is_dropped_without_an_error() {
    let server = Server::start(|base, request| match request.target.as_str() {
        "/feed" => {
            let mut release: serde_json::Value =
                serde_json::from_str(&github_release(base, "v2.0.0", ALL_ASSETS)).unwrap();
            release["published_at"] = "last tuesday".into();
            Reply::ok(release.to_string())
        }
        _ => Reply::ok(SIGNATURE_TEXT),
    });
    let update = check("1.0.0", &options(&[server.url("/feed")]))
        .unwrap()
        .unwrap();
    assert_eq!(update.date(), None);
    assert_eq!(update.notes(), Some("Changelog"));
}

// ---------------------------------------------------------------------------------------
// A5: validation, timeouts, request headers

#[test]
fn a5_invalid_inputs_are_rejected_before_any_request_is_made() {
    let server = Server::start(|_, _| Reply::status(204));
    let good = server.url("/feed");
    let with = |change: &dyn Fn(&mut CheckOptions)| {
        let mut options = options(std::slice::from_ref(&good));
        change(&mut options);
        options
    };

    let error = check("1.0.0", &with(&|o| o.endpoints.clear())).unwrap_err();
    assert!(matches!(error, Error::NoEndpoints), "{error}");

    let error = check("1.0.0", &with(&|o| o.pubkey = "  ".to_string())).unwrap_err();
    assert!(matches!(error, Error::MissingPublicKey), "{error}");

    for version in ["", "latest", "1.2", "1.2.3.4", "V1.0.0"] {
        let error = check(version, &with(&|_| {})).unwrap_err();
        assert!(
            matches!(error, Error::InvalidVersion { .. }),
            "{version}: {error}"
        );
    }

    for endpoint in ["not a url", "", "://missing-scheme", "http://exa mple.com"] {
        let error = check("1.0.0", &with(&|o| o.endpoints.push(endpoint.to_string()))).unwrap_err();
        assert!(
            matches!(error, Error::InvalidUrl { .. }),
            "{endpoint}: {error}"
        );
    }

    let error = check(
        "1.0.0",
        &with(&|o| o.headers = Some(vec![Header::new("bad name", "v")])),
    )
    .unwrap_err();
    assert!(matches!(error, Error::InvalidHeader { .. }), "{error}");
    let error = check(
        "1.0.0",
        &with(&|o| o.headers = Some(vec![Header::new("x-ok", "line\nbreak")])),
    )
    .unwrap_err();
    assert!(matches!(error, Error::InvalidHeader { .. }), "{error}");

    for mode in ["silent", "", "Basic", "QUIET"] {
        let error =
            check("1.0.0", &with(&|o| o.install_mode = Some(mode.to_string()))).unwrap_err();
        assert!(
            matches!(error, Error::InvalidInstallMode(_)),
            "{mode}: {error}"
        );
    }

    let error = check("1.0.0", &with(&|o| o.timeout_ms = Some(0))).unwrap_err();
    assert!(matches!(error, Error::InvalidTimeout), "{error}");

    assert!(
        server.requests().is_empty(),
        "requests: {:?}",
        server.requests()
    );
}

#[test]
fn a5_an_unsupported_platform_fails_explicitly() {
    for (os, arch) in [
        ("linux", "mips"),
        ("freebsd", "x86_64"),
        ("linux", "powerpc64"),
    ] {
        assert!(matches!(
            Platform::from_parts(os, arch),
            Err(Error::UnsupportedPlatform(_))
        ));
    }
}

#[test]
fn a5_install_mode_and_installer_args_are_validated_and_carried() {
    let server = feed_server(dynamic_manifest(
        "2.0.0",
        "http://127.0.0.1:1/p",
        "s",
        "appimage",
    ));
    let mut options = options(&[server.url("/feed")]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    assert_eq!(update.install_mode(), InstallMode::Passive);
    assert!(update.installer_args().is_empty());

    options.install_mode = Some("BasicUi".to_string());
    options.installer_args = Some(vec!["/S".to_string(), "/D=C:\\app".to_string()]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    assert_eq!(update.install_mode(), InstallMode::BasicUi);
    assert_eq!(update.installer_args(), ["/S", "/D=C:\\app"]);
}

#[test]
fn a5_the_request_timeout_expires_and_rejects() {
    let server = Server::start(|_, _| Reply::stalled(Duration::from_secs(4)));
    let mut options = options(&[server.url("/feed")]);
    options.timeout_ms = Some(250);
    let started = Instant::now();
    let error = check("1.0.0", &options).unwrap_err();
    assert!(matches!(error, Error::Request { .. }), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    // Without a timeout the same server answers (after its delay) within the test budget.
    let quick = Server::start(|_, _| Reply::stalled(Duration::from_millis(300)));
    let mut options = self::options(&[quick.url("/feed")]);
    options.timeout_ms = Some(5000);
    let error = check("1.0.0", &options).unwrap_err();
    assert!(
        matches!(error, Error::MissingField(_) | Error::MalformedFeed(_)),
        "{error}"
    );
}

#[test]
fn a5_the_timeout_applies_to_the_signature_and_package_requests() {
    let slow_signature = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, "v2.0.0", ALL_ASSETS)),
        _ => Reply::stalled(Duration::from_secs(4)),
    });
    let mut options = options(&[slow_signature.url("/feed")]);
    options.timeout_ms = Some(250);
    let started = Instant::now();
    let error = check("1.0.0", &options).unwrap_err();
    assert!(matches!(error, Error::Request { .. }), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));

    let slow_package = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(dynamic_manifest(
            "2.0.0",
            &format!("{base}/package"),
            "s",
            "appimage",
        )),
        _ => Reply::stalled(Duration::from_secs(4)),
    });
    let mut options = self::options(&[slow_package.url("/feed")]);
    options.timeout_ms = Some(250);
    let update = check("1.0.0", &options).unwrap().unwrap();
    assert_eq!(update.timeout(), Some(Duration::from_millis(250)));
    let started = Instant::now();
    let error = block_on(update.download_and_install()).unwrap_err();
    assert!(matches!(error, Error::Request { .. }), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn a5_request_headers_follow_the_defaults_and_the_callers_overrides() {
    let routes = |base: &str, request: &Recorded| match request.target.as_str() {
        "/feed" => Reply::ok(github_release(base, "v2.0.0", ALL_ASSETS)),
        _ => Reply::ok(SIGNATURE_TEXT),
    };

    // Defaults: JSON for the feed, octet-stream and the updater's user agent for downloads.
    let server = Server::start(routes);
    let mut options = options(&[server.url("/feed")]);
    options.headers = Some(vec![
        Header::new("X-Api-Key", "k"),
        Header::new("x-multi", "1"),
        Header::new("x-multi", "2"),
    ]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].header("accept"), Some("application/json"));
    assert_eq!(
        requests[1].header("accept"),
        Some("application/octet-stream")
    );
    assert_eq!(
        requests[1].header("user-agent"),
        Some(vespertine_updater::USER_AGENT_VALUE)
    );
    assert!(vespertine_updater::USER_AGENT_VALUE.starts_with("vespertine-updater/"));
    for request in &requests {
        assert_eq!(request.header("x-api-key"), Some("k"));
        assert_eq!(request.header_values("x-multi"), ["1", "2"]);
    }

    // The package download sends the same defaults and the caller's headers.
    block_on(update.download_and_install()).ok();
    let download = &server.requests()[2];
    assert_eq!(download.target, "/assets/app_x86_64.AppImage");
    assert_eq!(download.header("accept"), Some("application/octet-stream"));
    assert_eq!(
        download.header("user-agent"),
        Some(vespertine_updater::USER_AGENT_VALUE)
    );
    assert_eq!(download.header("x-api-key"), Some("k"));

    // Caller-supplied Accept and User-Agent replace the defaults.
    let server = Server::start(routes);
    let mut options = self::options(&[server.url("/feed")]);
    options.headers = Some(vec![
        Header::new("Accept", "application/vnd.test+json"),
        Header::new("User-Agent", "custom/1"),
    ]);
    let update = check("1.0.0", &options).unwrap().unwrap();
    block_on(update.download_and_install()).ok();
    for request in server.requests() {
        assert_eq!(
            request.header_values("accept"),
            ["application/vnd.test+json"],
            "{}",
            request.target
        );
    }
    let requests = server.requests();
    assert_eq!(requests[1].header_values("user-agent"), ["custom/1"]);
    assert_eq!(requests[2].header_values("user-agent"), ["custom/1"]);
}

#[test]
fn a5_certificate_variables_are_defaulted_before_the_first_request() {
    let environment = Arc::new(TestEnvironment::new("/nonexistent/app"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let server = {
        let (environment, seen) = (environment.clone(), seen.clone());
        Server::start(move |_, _| {
            seen.lock().unwrap().push((
                environment.current_var("SSL_CERT_FILE"),
                environment.current_var("SSL_CERT_DIR"),
            ));
            Reply::status(204)
        })
    };
    let updater = updater(environment.clone());
    assert!(
        block_on(updater.check("1.0.0", &options(&[server.url("/feed")])))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [(
            Some("/etc/ssl/certs/ca-certificates.crt".into()),
            Some("/etc/ssl/certs".into())
        )]
    );
    assert_eq!(
        environment.set_calls(),
        [
            "SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt",
            "SSL_CERT_DIR=/etc/ssl/certs"
        ]
    );
}

#[test]
fn a5_certificate_variables_that_are_already_set_are_left_alone() {
    let environment = Arc::new(
        TestEnvironment::new("/nonexistent/app").var("SSL_CERT_FILE", "/custom/bundle.pem"),
    );
    let server = Server::start(|_, _| Reply::status(204));
    let updater = updater(environment.clone());
    block_on(updater.check("1.0.0", &options(&[server.url("/feed")]))).unwrap();
    assert_eq!(environment.set_calls(), ["SSL_CERT_DIR=/etc/ssl/certs"]);
    assert_eq!(
        environment.current_var("SSL_CERT_FILE"),
        Some("/custom/bundle.pem".into())
    );

    // Invalid input fails before the environment is touched.
    let environment = Arc::new(TestEnvironment::new("/nonexistent/app"));
    let updater = self::updater(environment.clone());
    assert!(block_on(updater.check("nope", &options(&[server.url("/feed")]))).is_err());
    assert!(environment.set_calls().is_empty());
}
