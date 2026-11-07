//! Remote (streaming) library folders — MEGA links (mirrors `remoteFolders.ts`).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::types::{DirTarget, FileRef, FolderRef, Library, LinkKind, Progress, RemoteProvider};

/// State of a remote folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteFolderState {
    /// listing the tree
    Listing,
    /// ready
    Ok,
    /// unreachable
    Offline,
    /// transfer quota exceeded
    Quota,
    /// failed
    Error,
}

/// Why a remote folder is not 'ok'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteErrorCode {
    /// link removed
    NotFound,
    /// blocked by MEGA
    Blocked,
    /// wrong key
    Key,
    /// network problem
    Network,
    /// quota
    Quota,
    /// temporarily unavailable
    Unavailable,
    /// anything else
    Other,
}

/// Status of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFolderStatus {
    /// state
    pub state: RemoteFolderState,
    /// English detail
    pub error: Option<String>,
    /// error code
    pub code: Option<RemoteErrorCode>,
    /// ms epoch: quota reset / next retry
    pub retry_at: Option<i64>,
    /// ms epoch of the last successful listing
    pub listed_at: Option<i64>,
}

/// `FolderRef.remote`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFolderInfo {
    /// status fields
    #[serde(flatten)]
    pub status: RemoteFolderStatus,
    /// provider
    pub provider: RemoteProvider,
    /// the link WITHOUT its key, e.g. "mega.nz/folder/AbCd1234"
    pub label: String,
    /// folder or single file link
    pub link_kind: LinkKind,
    /// background metadata + thumbnail progress
    pub indexing: Progress,
}

/// `POST /api/remote/folders`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFolderCreate {
    /// link
    pub url: String,
    /// add to this library
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    /// new library name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_name: Option<String>,
    /// folder display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Result of `POST /api/remote/folders`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteFolderCreated {
    /// library
    pub library: Library,
    /// folder
    pub folder: FolderRef,
}

/// `POST /api/remote/copy`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCopySpec {
    /// media ids
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// files / dirs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<FileRef>>,
    /// target
    pub to: DirTarget,
}

/// Default remote stream cache cap.
pub const DEFAULT_REMOTE_CACHE_MB: i64 = 2048;

/// MEGA account type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MegaAccountType {
    /// free
    Free,
    /// Pro I–III, Lite, Flexi
    Pro,
    /// business
    Business,
}

/// Account transfer quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MegaTransfer {
    /// bytes used
    pub used: i64,
    /// bytes total
    pub total: Option<i64>,
}

/// `GET /api/remote/account`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MegaAccountStatus {
    /// logged in
    pub logged_in: bool,
    /// email
    pub email: Option<String>,
    /// account type
    #[serde(rename = "type")]
    pub kind: Option<MegaAccountType>,
    /// plan name
    pub plan: Option<String>,
    /// transfer quota
    pub transfer: Option<MegaTransfer>,
    /// the stored session was rejected
    pub expired: bool,
    /// ms epoch of the login
    pub since: Option<i64>,
}

/// `POST /api/remote/account/login`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MegaLoginRequest {
    /// email
    pub email: String,
    /// password
    pub password: String,
    /// two-factor code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Login refusal reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MegaLoginRefusal {
    /// wrong email or password
    Credentials,
    /// two-factor code needed
    MfaRequired,
    /// wrong two-factor code
    MfaInvalid,
    /// account blocked
    Blocked,
}

/// `{ ok: true, account } | { ok: false, reason }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MegaLoginResult {
    /// logged in
    Ok(MegaAccountStatus),
    /// refused
    Refused(MegaLoginRefusal),
}

impl Serialize for MegaLoginResult {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Okay<'a> {
            ok: bool,
            account: &'a MegaAccountStatus,
        }
        #[derive(Serialize)]
        struct Refused {
            ok: bool,
            reason: MegaLoginRefusal,
        }
        match self {
            MegaLoginResult::Ok(a) => Okay {
                ok: true,
                account: a,
            }
            .serialize(s),
            MegaLoginResult::Refused(r) => Refused {
                ok: false,
                reason: *r,
            }
            .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for MegaLoginResult {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            ok: bool,
            account: Option<MegaAccountStatus>,
            reason: Option<MegaLoginRefusal>,
        }
        let r = Raw::deserialize(d)?;
        match (r.ok, r.account, r.reason) {
            (true, Some(a), _) => Ok(MegaLoginResult::Ok(a)),
            (false, _, Some(x)) => Ok(MegaLoginResult::Refused(x)),
            _ => Err(serde::de::Error::custom("invalid MegaLoginResult")),
        }
    }
}
