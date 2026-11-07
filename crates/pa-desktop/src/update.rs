//! Release check against GitHub (shell.ts `checkForUpdate`): never downloads anything, only tells
//! the user (toast with a link) once per version. Auto check: at most once a day.

use pa_ui::host::UpdateInfo;

/// the Electron app's endpoints (GitHub redirects renamed repositories)
pub const RELEASES_API: &str = "https://api.github.com/repos/zearkreast/PeakAround/releases/latest";
/// fallback release page
pub const RELEASES_PAGE: &str = "https://github.com/zearkreast/PeakAround/releases/latest";

struct V {
    n: [u64; 3],
    pre: String,
}

fn parse(v: &str) -> V {
    let s = v.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let (core, pre) = match s.find('-') {
        Some(i) => (&s[..i], s[i + 1..].to_string()),
        None => (s, String::new()),
    };
    let mut n = [0u64; 3];
    let mut ok = false;
    for (i, part) in core.split('.').take(3).enumerate() {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        n[i] = digits.parse().unwrap_or(0);
        ok = true;
    }
    if !ok {
        return V {
            n: [0; 3],
            pre: String::new(),
        };
    }
    // `[0-9a-z.-]+` only
    let pre: String = pre
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .collect();
    V { n, pre }
}

/// numeric-aware compare of pre-release tags (`localeCompare(…, { numeric: true })`)
fn cmp_pre(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(c) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(c);
                    y.next();
                }
                let o = n1
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(&n2.parse::<u64>().unwrap_or(0));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase());
                if o.is_ne() {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// "1.2.0", "v1.10.1", "1.2.0-beta.1" (a pre-release sorts before its release)
pub fn compare_semver(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (parse(a), parse(b));
    for i in 0..3 {
        if x.n[i] != y.n[i] {
            return x.n[i].cmp(&y.n[i]);
        }
    }
    if x.pre == y.pre {
        return std::cmp::Ordering::Equal;
    }
    if x.pre.is_empty() {
        return std::cmp::Ordering::Greater;
    }
    if y.pre.is_empty() {
        return std::cmp::Ordering::Less;
    }
    cmp_pre(&x.pre, &y.pre)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// latest GitHub release vs `current`
pub async fn check(current: &str) -> UpdateInfo {
    let checked_at = now_ms();
    let fail = |e: String| UpdateInfo {
        current: current.to_string(),
        latest: None,
        newer: false,
        url: None,
        checked_at,
        error: Some(e),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("PeekAround/{current}"))
        .build()
    {
        Ok(c) => c,
        Err(e) => return fail(e.to_string()),
    };
    let res = match client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    if res.status().as_u16() == 404 {
        return UpdateInfo {
            current: current.into(),
            checked_at,
            ..Default::default()
        };
    }
    if !res.status().is_success() {
        return fail(format!("GitHub {}", res.status().as_u16()));
    }
    let j: serde_json::Value = match res.json().await {
        Ok(j) => j,
        Err(e) => return fail(e.to_string()),
    };
    evaluate(current, &j, checked_at)
}

/// the release JSON → UpdateInfo
pub fn evaluate(current: &str, j: &serde_json::Value, checked_at: i64) -> UpdateInfo {
    let latest = j["tag_name"]
        .as_str()
        .map(|t| t.strip_prefix(['v', 'V']).unwrap_or(t).to_string());
    let url = j["html_url"]
        .as_str()
        .filter(|u| u.starts_with("https://github.com/"))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    let newer = latest.as_deref().is_some_and(|l| {
        !j["draft"].as_bool().unwrap_or(false)
            && !j["prerelease"].as_bool().unwrap_or(false)
            && compare_semver(l, current).is_gt()
    });
    UpdateInfo {
        current: current.into(),
        latest,
        newer,
        url: Some(url),
        checked_at,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn semver() {
        assert_eq!(compare_semver("1.2.0", "1.2.0"), Equal);
        assert_eq!(compare_semver("v1.10.1", "1.9.9"), Greater);
        assert_eq!(compare_semver("1.2.0-beta.1", "1.2.0"), Less);
        assert_eq!(compare_semver("1.2.0", "1.2.0-beta.1"), Greater);
        assert_eq!(compare_semver("1.2.0-beta.10", "1.2.0-beta.9"), Greater);
        assert_eq!(compare_semver("2.0.0-alpha.1", "1.3.2"), Greater);
        assert_eq!(compare_semver("1.3", "1.3.0"), Equal);
        assert_eq!(compare_semver("garbage", "0.0.0"), Equal);
    }

    #[test]
    fn evaluates_release() {
        let j = serde_json::json!({"tag_name":"v2.1.0","html_url":"https://github.com/zearkreast/PeakAround/releases/tag/v2.1.0","draft":false,"prerelease":false});
        let u = evaluate("2.0.0", &j, 1);
        assert!(u.newer);
        assert_eq!(u.latest.as_deref(), Some("2.1.0"));
        let j = serde_json::json!({"tag_name":"v2.1.0","html_url":"https://evil.example/x","prerelease":true});
        let u = evaluate("2.0.0", &j, 1);
        assert!(!u.newer);
        assert_eq!(u.url.as_deref(), Some(RELEASES_PAGE));
    }
}
