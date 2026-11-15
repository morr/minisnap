//! Upload providers: the saved shot file goes to an image hosting backend.
//!
//! The mechanism is a trait boundary (`UploadBackend`); the Artifacts
//! server is the first implementation. A future hosting provider adds a
//! backend, a config section and a preferences row without touching the
//! callers. The shot file is always current on disk (`OpenShot::persist`
//! rewrites it after every edit), so an upload always ships the visible
//! state of the shot.

use std::fs;
use std::iter;
use std::path::Path;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::{msg_send, ClassType, Message};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSURL, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSString,
};

use crate::app::MainJob;
use crate::config::{ArtifactsProviderConfig, Config};

/// Longest server-side image name — the server rule is
/// `^[a-z0-9][a-z0-9-]{0,63}$`.
const NAME_MAX: usize = 64;

/// Local prefix of a shot file name, swapped for [`NAME_PREFIX`].
const LOCAL_PREFIX: &str = "MiniSnap_";

/// Server-side name prefix.
const NAME_PREFIX: &str = "shot-";

/// One upload destination. Implementations send the request off the main
/// thread and report the outcome as a `MainJob::UploadFinished` job on
/// `jobs` — the tick pumps it back onto the main thread.
pub(crate) trait UploadBackend {
    fn upload(&self, png: Vec<u8>, name: &str, jobs: Sender<MainJob>);
}

/// The Artifacts server backend: `PUT /api/images/{name}` with the PNG as
/// the body, create-only (`X-Create-Only: true`); a 409 retries with the
/// next `-2`…`-9` suffix. Transport — the system NSURLSession; each
/// completion lands on the session's background queue.
pub(crate) struct ArtifactsBackend {
    base_url: String,
    token: String,
    project: String,
}

impl ArtifactsBackend {
    pub(crate) fn new(provider: &ArtifactsProviderConfig) -> Self {
        ArtifactsBackend {
            base_url: provider.base_url.clone(),
            token: provider.token.clone(),
            project: provider.project.clone(),
        }
    }
}

impl UploadBackend for ArtifactsBackend {
    fn upload(&self, png: Vec<u8>, name: &str, jobs: Sender<MainJob>) {
        let names: Vec<String> = iter::once(name.to_string())
            .chain(retry_names(name))
            .collect();
        let data = NSData::from_vec(png);
        // A session per upload: the action is user-initiated and one at a
        // time, and the completion block holds the session (and the body
        // data) for as long as the chain is in flight.
        let config = NSURLSessionConfiguration::defaultSessionConfiguration();
        let session = unsafe {
            NSURLSession::sessionWithConfiguration_delegate_delegateQueue(&config, None, None)
        };
        upload_attempt(&session, &data, self, names, jobs);
    }
}

/// The connection parameters shared by every attempt of one upload chain.
struct Attempt<'a> {
    backend: &'a ArtifactsBackend,
    data: &'a NSData,
    session: &'a NSURLSession,
}

/// One PUT attempt with `names[0]`; on 409 the rest of the names are
/// retried by a fresh attempt. The completion block captures the session
/// and the body, so they outlive this call while the task is in flight.
fn upload_attempt(
    session: &NSURLSession,
    data: &NSData,
    backend: &ArtifactsBackend,
    names: Vec<String>,
    jobs: Sender<MainJob>,
) {
    let Some(name) = names.first() else {
        let _ = jobs.send(MainJob::UploadFinished {
            result: Err("upload: every name attempt is taken".to_string()),
        });
        return;
    };
    let rest = names[1..].to_vec();

    let url_string = format!("{}/api/images/{name}", backend.base_url.trim_end_matches('/'));
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&url_string)) else {
        let _ = jobs.send(MainJob::UploadFinished {
            result: Err(format!("upload: bad server url {url_string:?}")),
        });
        return;
    };
    let request = NSMutableURLRequest::requestWithURL(&url);
    request.setHTTPMethod(&NSString::from_str("PUT"));
    // A stuck server must not leave the editor tile spinning forever.
    request.setTimeoutInterval(30.0);
    let auth = NSString::from_str(&format!("Bearer {}", backend.token));
    request.setValue_forHTTPHeaderField(Some(&auth), &NSString::from_str("Authorization"));
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str("image/png")),
        &NSString::from_str("Content-Type"),
    );
    let project = NSString::from_str(&backend.project);
    request.setValue_forHTTPHeaderField(Some(&project), &NSString::from_str("X-Project"));
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str("true")),
        &NSString::from_str("X-Create-Only"),
    );
    request.setHTTPBody(Some(data));

    let retained_session = session.retain();
    let retained_data = data.retain();
    let jobs = jobs.clone();
    let base_url = backend.base_url.clone();
    let token = backend.token.clone();
    let project = backend.project.clone();
    let handler = RcBlock::new(
        move |body: *mut NSData,
              response: *mut NSURLResponse,
              error: *mut NSError|
              -> () {
            match unsafe { classify_reply(body, response, error) } {
                UploadReply::Created(url) => {
                    let _ = jobs.send(MainJob::UploadFinished { result: Ok(url) });
                }
                UploadReply::Conflict => {
                    if rest.is_empty() {
                        let _ = jobs.send(MainJob::UploadFinished {
                            result: Err("upload: every name attempt is taken".to_string()),
                        });
                    } else {
                        let cfg = ArtifactsBackend {
                            base_url: base_url.clone(),
                            token: token.clone(),
                            project: project.clone(),
                        };
                        upload_attempt(&retained_session, &retained_data, &cfg, rest.clone(), jobs.clone());
                    }
                }
                UploadReply::Failed(message) => {
                    let _ = jobs.send(MainJob::UploadFinished { result: Err(message) });
                }
            }
        },
    );
    let task = unsafe {
        session.uploadTaskWithRequest_fromData_completionHandler(&request, Some(data), &handler)
    };
    task.resume();
}

/// Classify the raw URLSession completion parameters via [`parse_reply`].
///
/// # Safety
/// The pointers are the completion block's parameters, valid for the call.
unsafe fn classify_reply(
    body: *mut NSData,
    response: *mut NSURLResponse,
    error: *mut NSError,
) -> UploadReply {
    if !error.is_null() {
        let message = unsafe { (*error).localizedDescription().to_string() };
        return UploadReply::Failed(format!("upload failed: {message}"));
    }
    let status = unsafe {
        response.as_ref().and_then(|r| {
            let is_http: bool = msg_send![r, isKindOfClass: NSHTTPURLResponse::class()];
            is_http.then(|| {
                let code: isize = msg_send![r, statusCode];
                code as u16
            })
        })
    };
    let Some(status) = status else {
        return UploadReply::Failed("upload: no HTTP response".to_string());
    };
    let text = unsafe { body.as_ref() }
        .map(|data| String::from_utf8_lossy(&data.to_vec()).into_owned())
        .unwrap_or_default();
    parse_reply(status, &text)
}

/// Server-side image name derived from a shot file stem: the local
/// `MiniSnap_2026-11-15_14-39-59@2x_2` becomes `shot-2026-11-15-14-39-59`.
/// The density marker and the local collision suffix are dropped — a local
/// `_2` means "the same second", which the server uniques with its own
/// `-2`. The result satisfies the server name rule regardless of the stem.
pub(crate) fn upload_name(stem: &str) -> String {
    let mut base = strip_collision_suffix(stem);
    if let Some(at) = base.find('@') {
        base = &base[..at];
    }
    let body = base.strip_prefix(LOCAL_PREFIX).unwrap_or(base);
    let mut out = String::from(NAME_PREFIX);
    for ch in body.chars() {
        if out.len() >= NAME_MAX {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out
}

/// A trailing `_2`…`_999` marks a same-second collision; any other tail
/// (including the timestamp's own digits after an underscore) stays.
fn strip_collision_suffix(stem: &str) -> &str {
    if let Some(under) = stem.rfind('_') {
        let digits = &stem[under + 1..];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return &stem[..under];
        }
    }
    stem
}

/// Create-only retry names for a base name: `-2`…`-9`, after which the
/// names are exhausted and the upload fails.
fn retry_names(base: &str) -> impl Iterator<Item = String> + '_ {
    (2..=9).map(move |n| format!("{base}-{n}"))
}

/// The parsed reply of one upload attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UploadReply {
    /// 201/200: the image is in; carries the public URL of the image.
    Created(String),
    /// 409: the name is taken; the caller retries with the next suffix.
    Conflict,
    /// Anything else: a readable message for the user.
    Failed(String),
}

/// Classify a server reply. A successful body is JSON whose `url` field is
/// the public image address; failures surface the server's `error`/`message`
/// text, falling back to the raw body or just the status code.
pub(crate) fn parse_reply(status: u16, body: &str) -> UploadReply {
    let json = || serde_json::from_str::<serde_json::Value>(body).ok();
    match status {
        200 | 201 => match json().and_then(|v| url_field(&v).map(str::to_string)) {
            Some(url) => UploadReply::Created(url),
            None => UploadReply::Failed(format!("server replied {status} without an image url")),
        },
        409 => UploadReply::Conflict,
        _ => {
            let detail = json()
                .and_then(|v| text_field(&v))
                .map(|text| format!("server error {status}: {text}"))
                .unwrap_or_else(|| match body.trim() {
                    "" => format!("server replied {status}"),
                    trimmed => {
                        let mut short: String = trimmed.chars().take(120).collect();
                        if trimmed.chars().count() > 120 {
                            short.push('…');
                        }
                        short
                    }
                });
            UploadReply::Failed(detail)
        }
    }
}

/// The `url` field of a success body.
fn url_field(v: &serde_json::Value) -> Option<&str> {
    v.get("url")?.as_str()
}

/// The first non-empty of `error`/`message` in a failure body.
fn text_field(v: &serde_json::Value) -> Option<String> {
    ["error", "message"]
        .into_iter()
        .find_map(|key| v.get(key)?.as_str().map(str::to_string))
        .filter(|text| !text.trim().is_empty())
}

/// The backend of the active enabled provider; `None` when upload is not
/// configured — the provider is off, unknown, or its token is empty. The
/// callers answer `None` with the "configure upload" dialog (the capture
/// stream just logs: it has no window to hang a dialog on).
pub(crate) fn build_backend(config: &Config) -> Option<Box<dyn UploadBackend>> {
    if config.upload_provider != "artifacts" {
        return None;
    }
    let provider = &config.upload.artifacts;
    if !provider.enabled || provider.token.trim().is_empty() {
        return None;
    }
    Some(Box::new(ArtifactsBackend::new(provider)))
}

/// Upload a saved shot file through `backend`: read the PNG, derive the
/// server name from the file stem and hand the request off. Only local
/// failures come back as `Err`; the backend reports its own outcome
/// through `jobs`.
pub(crate) fn upload_file(
    backend: &dyn UploadBackend,
    path: &Path,
    jobs: Sender<MainJob>,
) -> Result<(), String> {
    let png = fs::read(path).map_err(|e| format!("upload: cannot read {}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| format!("upload: {} has no file name", path.display()))?;
    backend.upload(png, &upload_name(&stem), jobs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn backend_requires_enabled_provider_with_a_token() {
        let mut cfg = crate::config::Config::default_with(PathBuf::from("/tmp/pictures"));
        assert!(build_backend(&cfg).is_none(), "the provider is off by default");
        cfg.upload.artifacts.enabled = true;
        assert!(build_backend(&cfg).is_none(), "no token yet");
        cfg.upload.artifacts.token = "secret".to_string();
        assert!(build_backend(&cfg).is_some(), "enabled with a token");
        cfg.upload_provider = "gdrive".to_string();
        assert!(build_backend(&cfg).is_none(), "unknown active provider");
    }

    #[test]
    fn plain_stem_loses_the_prefix_and_keeps_the_timestamp() {
        assert_eq!(
            upload_name("MiniSnap_2026-11-15_14-39-59"),
            "shot-2026-11-15-14-39-59"
        );
    }

    #[test]
    fn density_marker_is_dropped() {
        assert_eq!(
            upload_name("MiniSnap_2026-11-15_14-39-59@2x"),
            "shot-2026-11-15-14-39-59"
        );
    }

    #[test]
    fn collision_suffix_is_dropped_after_the_marker() {
        assert_eq!(
            upload_name("MiniSnap_2026-11-15_14-39-59@2x_2"),
            "shot-2026-11-15-14-39-59"
        );
    }

    #[test]
    fn collision_suffix_is_dropped_without_a_marker() {
        assert_eq!(
            upload_name("MiniSnap_2026-11-15_14-39-59_7"),
            "shot-2026-11-15-14-39-59"
        );
    }

    #[test]
    fn timestamp_digits_are_not_mistaken_for_a_collision_suffix() {
        assert_eq!(
            upload_name("MiniSnap_2026-11-15_14-39-59"),
            "shot-2026-11-15-14-39-59"
        );
    }

    #[test]
    fn long_stem_is_clamped_to_the_server_limit() {
        let name = upload_name(&format!("MiniSnap_{}", "a".repeat(200)));
        assert_eq!(name.len(), NAME_MAX);
        assert!(name.starts_with("shot-"));
    }

    #[test]
    fn every_result_satisfies_the_server_rule() {
        for stem in [
            "MiniSnap_2026-11-15_14-39-59@2x_2",
            "MiniSnap_notes",
            "Привидение_2026-11-15",
            "shot",
            "",
        ] {
            let name = upload_name(stem);
            assert!(name.len() <= NAME_MAX, "{stem:?} → {name:?}");
            let mut chars = name.chars();
            let first = chars.next().unwrap();
            assert!(first.is_ascii_digit() || first.is_ascii_lowercase());
            assert!(chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }

    #[test]
    fn retries_run_from_2_to_9() {
        let base = "shot-2026-11-15-14-39-59";
        let retries: Vec<_> = retry_names(base).collect();
        assert_eq!(retries.len(), 8);
        assert_eq!(retries[0], "shot-2026-11-15-14-39-59-2");
        assert_eq!(retries[7], "shot-2026-11-15-14-39-59-9");
    }

    #[test]
    fn success_body_yields_the_public_url() {
        let body = r#"{"url":"https://artifacts.example/i/minisnap/shot-2026-11-15-14-39-59.png"}"#;
        assert_eq!(
            parse_reply(201, body),
            UploadReply::Created("https://artifacts.example/i/minisnap/shot-2026-11-15-14-39-59.png".into())
        );
        assert_eq!(
            parse_reply(200, r#"{"stored":true,"url":"/i/p/x.png"}"#),
            UploadReply::Created("/i/p/x.png".into())
        );
    }

    #[test]
    fn success_without_a_url_is_a_failure() {
        assert_eq!(
            parse_reply(201, "{}"),
            UploadReply::Failed("server replied 201 without an image url".into())
        );
    }

    #[test]
    fn conflict_is_not_a_failure() {
        assert_eq!(
            parse_reply(409, r#"{"error":"name already exists"}"#),
            UploadReply::Conflict
        );
    }

    #[test]
    fn failure_body_text_is_surfaced() {
        assert_eq!(
            parse_reply(401, r#"{"error":"invalid token"}"#),
            UploadReply::Failed("server error 401: invalid token".into())
        );
        assert_eq!(
            parse_reply(403, r#"{"message":"project not found"}"#),
            UploadReply::Failed("server error 403: project not found".into())
        );
    }

    #[test]
    fn failure_without_a_body_falls_back_to_the_status() {
        assert_eq!(parse_reply(500, ""), UploadReply::Failed("server replied 500".into()));
        assert_eq!(
            parse_reply(502, "<html>bad gateway</html>"),
            UploadReply::Failed("<html>bad gateway</html>".into())
        );
    }
}
