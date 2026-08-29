//! AEM CRX Package Manager HTTP client (AEM 6.5, HTTP basic auth).
//!
//! Uploads a generated FileVault package to an AEM instance and installs it via
//! the Package Manager service API:
//!
//! - upload:  `POST {host}/crx/packmgr/service/.json/?cmd=upload` (multipart)
//! - install: `POST {host}/crx/packmgr/service/.json{path}?cmd=install`

use blueprint::AemConnection;

/// Upload and install a FileVault package on the configured AEM instance.
///
/// `zip` is the raw package bytes, `package_name` becomes the uploaded file
/// name (`{package_name}.zip`). Returns `Ok(())` on a successful install, or an
/// `Err` carrying the CRX message / HTTP status on failure.
pub async fn upload_and_install_package(
    conn: &AemConnection,
    zip: Vec<u8>,
    package_name: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let host = conn.host.trim_end_matches('/');

    // ── Upload ──────────────────────────────────────────────────────────
    let file_name = format!("{package_name}.zip");
    let part = reqwest::multipart::Part::bytes(zip)
        .file_name(file_name)
        .mime_str("application/zip")
        .map_err(|e| format!("Failed to build upload part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("package", part)
        .text("force", "true");

    let upload_url = format!("{host}/crx/packmgr/service/.json/?cmd=upload");
    let resp = client
        .post(&upload_url)
        .basic_auth(&conn.username, Some(&conn.password))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("AEM upload request failed: {e}"))?;

    let path = parse_crx_response(resp, "upload")
        .await?
        .ok_or_else(|| "AEM upload succeeded but returned no package path".to_string())?;

    // ── Install ─────────────────────────────────────────────────────────
    let install_url = format!("{host}/crx/packmgr/service/.json{path}?cmd=install");
    let resp = client
        .post(&install_url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM install request failed: {e}"))?;

    parse_crx_response(resp, "install").await?;
    Ok(())
}

/// Parse a CRX Package Manager `.json` response.
///
/// Returns the package `path` (present on upload) on success. CRX returns an
/// HTML login page on auth failure, so a non-JSON body or a non-success status
/// is surfaced as a clear error.
async fn parse_crx_response(
    resp: reqwest::Response,
    action: &str,
) -> Result<Option<String>, String> {
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("AEM {action} response read failed: {e}"))?;

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
        if status.as_u16() == 401 || status.as_u16() == 403 {
            format!("AEM {action} failed: authentication rejected (HTTP {status})")
        } else {
            let snippet: String = body.chars().take(200).collect();
            format!("AEM {action} failed (HTTP {status}): {snippet}")
        }
    })?;

    if json["success"].as_bool() == Some(true) {
        return Ok(json["path"].as_str().map(String::from));
    }

    let msg = json["msg"].as_str().unwrap_or("unknown error");
    Err(format!("AEM {action} failed: {msg}"))
}

/// Fetch the rendered Adaptive Form HTML from AEM for verification.
///
/// `form_jcr_path` is the form's JCR node path (e.g.
/// `/content/forms/af/<form_path>/<form_dir>`); the form renders at that path
/// with an `.html` extension. Returns the HTML body.
pub async fn fetch_form_html(conn: &AemConnection, form_jcr_path: &str) -> Result<String, String> {
    let host = conn.host.trim_end_matches('/');
    let path = form_jcr_path.trim_end_matches('/');
    let url = format!("{host}{path}.html");
    let resp = reqwest::Client::new()
        .get(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM form fetch failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("AEM form fetch read failed: {e}"))?;
    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("AEM form fetch failed (HTTP {status}): {snippet}"));
    }
    Ok(body)
}

/// Fetch the Document-of-Record (DoR) PDF for a deployed form from AEM.
///
/// Uses the Adaptive Form guide-container DoR selector
/// (`{form}/jcr:content/guideContainer.af.dor.pdf`). Returns the raw PDF bytes;
/// errors (with a body snippet) if the response isn't a PDF — e.g. DoR isn't
/// configured for the form. (Confirm the exact selector against your AEM
/// version's DoR docs if this 404s.)
pub async fn fetch_dor_pdf(conn: &AemConnection, form_jcr_path: &str) -> Result<Vec<u8>, String> {
    let host = conn.host.trim_end_matches('/');
    let path = form_jcr_path.trim_end_matches('/');
    let url = format!("{host}{path}/jcr:content/guideContainer.af.dor.pdf");
    let resp = reqwest::Client::new()
        .get(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM DoR fetch failed: {e}"))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("AEM DoR read failed: {e}"))?
        .to_vec();
    if !status.is_success() {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(format!("AEM DoR fetch failed (HTTP {status}): {snippet}"));
    }
    if !bytes.starts_with(b"%PDF") {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(format!(
            "AEM DoR response was not a PDF (DoR may not be configured for this form): {snippet}"
        ));
    }
    Ok(bytes)
}

/// Log in to AEM the way a browser does and return the `login-token` cookie.
///
/// The rendered form and its preview are behind AEM's form login, which does
/// not accept HTTP basic auth the way the Package Manager does, which is why a
/// basic-auth GET of the `.html` render tends to 401. Posting the credentials to
/// `j_security_check` yields the session cookie a browser would hold, which the
/// browser session is then seeded with. `j_validate=true` makes AEM answer with
/// a status instead of a redirect.
pub async fn aem_login(conn: &AemConnection) -> Result<String, String> {
    let host = conn.host.trim_end_matches('/');
    let url = format!("{host}/libs/granite/core/content/login.html/j_security_check");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("AEM login client failed: {e}"))?;
    let resp = client
        .post(&url)
        .form(&[
            ("j_username", conn.username.as_str()),
            ("j_password", conn.password.as_str()),
            ("j_validate", "true"),
        ])
        .send()
        .await
        .map_err(|e| format!("AEM login request failed ({url}): {}", error_chain(&e)))?;
    let status = resp.status();
    let token = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(login_token_from_set_cookie);
    match token {
        Some(token) if status.is_success() => Ok(token),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            Err(format!(
                "AEM login rejected for user {:?} at {host} (HTTP {status}): {snippet}",
                conn.username
            ))
        }
    }
}

/// An error with its causes, innermost last: reqwest's top-level message
/// ("error sending request") says nothing without the "connection refused"
/// underneath it.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if parts.last() != Some(&text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(": ")
}

/// The value of a `login-token` cookie in one `Set-Cookie` header, if that is
/// the cookie the header sets.
pub fn login_token_from_set_cookie(header: &str) -> Option<String> {
    let (name, rest) = header.split_once('=')?;
    if name.trim() != "login-token" {
        return None;
    }
    let value = rest.split(';').next()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_token_is_read_from_its_set_cookie_header() {
        assert_eq!(
            login_token_from_set_cookie("login-token=abc.def; Path=/; HttpOnly"),
            Some("abc.def".to_string())
        );
        assert_eq!(
            login_token_from_set_cookie("sling.formauth=x; Path=/"),
            None
        );
        assert_eq!(login_token_from_set_cookie("login-token=; Path=/"), None);
        assert_eq!(login_token_from_set_cookie("garbage"), None);
    }
}
