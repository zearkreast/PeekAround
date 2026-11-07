//! SQLite database (mirrors `db.ts` + `migrations/*`): the same file, schema, migrations 001–005,
//! `PRAGMA user_version`, `.bak-v<N>` backups and SQL functions as the TypeScript server, so an
//! existing `peekaround.db` keeps working.
//!
//! The main connection (writes + short reads) sits behind a mutex; long reads use a small pool of
//! read-only connections (WAL: consistent snapshots that never block the writer).

use crate::geo::GeoService;
use parking_lot::Mutex;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, Row};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use rusqlite::params;
pub use rusqlite::Result as SqlResult;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS libraries (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  color TEXT NOT NULL,
  icon TEXT NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  temp INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  source_name TEXT,
  sort INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS media (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  dir TEXT NOT NULL,
  name TEXT NOT NULL,
  ext TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  width INTEGER,
  height INTEGER,
  duration REAL,
  taken_at INTEGER NOT NULL DEFAULT 0,
  thumbhash TEXT,
  favorite INTEGER NOT NULL DEFAULT 0,
  playback TEXT NOT NULL DEFAULT 'direct',
  meta_state INTEGER NOT NULL DEFAULT 0,
  thumb_state INTEGER NOT NULL DEFAULT 0,
  extra TEXT,
  seen INTEGER NOT NULL DEFAULT 0,
  UNIQUE (folder_id, dir, name)
);
CREATE INDEX IF NOT EXISTS media_dir ON media (folder_id, dir);
CREATE INDEX IF NOT EXISTS media_taken ON media (taken_at);
CREATE INDEX IF NOT EXISTS media_meta ON media (meta_state);
CREATE INDEX IF NOT EXISTS media_thumb ON media (thumb_state);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fonts (
  id INTEGER PRIMARY KEY,
  family TEXT NOT NULL,
  subfamily TEXT NOT NULL,
  weight INTEGER NOT NULL,
  style TEXT NOT NULL,
  format TEXT NOT NULL,
  file TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
"#;

/// A dynamic SQL parameter list.
pub type Params = Vec<Value>;

/// Convert into a SQL value (shorthand for building [`Params`]).
pub trait Sql {
    fn sql(self) -> Value;
}
impl Sql for i64 {
    fn sql(self) -> Value {
        Value::Integer(self)
    }
}
impl Sql for i32 {
    fn sql(self) -> Value {
        Value::Integer(self as i64)
    }
}
impl Sql for u32 {
    fn sql(self) -> Value {
        Value::Integer(self as i64)
    }
}
impl Sql for usize {
    fn sql(self) -> Value {
        Value::Integer(self as i64)
    }
}
impl Sql for f64 {
    fn sql(self) -> Value {
        Value::Real(self)
    }
}
impl Sql for bool {
    fn sql(self) -> Value {
        Value::Integer(self as i64)
    }
}
impl Sql for &str {
    fn sql(self) -> Value {
        Value::Text(self.to_string())
    }
}
impl Sql for String {
    fn sql(self) -> Value {
        Value::Text(self)
    }
}
impl Sql for &String {
    fn sql(self) -> Value {
        Value::Text(self.clone())
    }
}
impl<T: Sql> Sql for Option<T> {
    fn sql(self) -> Value {
        match self {
            Some(v) => v.sql(),
            None => Value::Null,
        }
    }
}
impl Sql for Value {
    fn sql(self) -> Value {
        self
    }
}
impl Sql for pa_meta::SqlValue {
    fn sql(self) -> Value {
        match self {
            pa_meta::SqlValue::Null => Value::Null,
            pa_meta::SqlValue::Int(i) => Value::Integer(i),
            pa_meta::SqlValue::Real(f) => Value::Real(f),
            pa_meta::SqlValue::Text(s) => Value::Text(s),
        }
    }
}

/// `p![a, b, c]` → [`Params`]
#[macro_export]
macro_rules! p {
    () => { Vec::<rusqlite::types::Value>::new() };
    ($($x:expr),+ $(,)?) => { vec![$($crate::db::Sql::sql($x)),+] };
}

/// The database: main connection + read-only pool.
pub struct Db {
    conn: Mutex<Option<Connection>>,
    readers: Mutex<Vec<Connection>>,
    file: PathBuf,
    geo: Arc<GeoService>,
    closed: std::sync::atomic::AtomicBool,
}

/// Error used after `close()`.
fn closed_err() -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName("database is not open".into())
}

impl Db {
    /// Open (create) the database file, apply the schema + pending migrations.
    pub fn open(file: &Path, geo: Arc<GeoService>) -> anyhow_like::Result<Db> {
        let memory = file.as_os_str() == ":memory:";
        if !memory {
            if let Some(d) = file.parent() {
                std::fs::create_dir_all(d)?;
            }
        }
        let conn = Connection::open(file)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        register_functions(&conn, &geo)?;
        conn.set_prepared_statement_cache_capacity(256);
        migrate(&conn, file, memory)?;
        Ok(Db {
            conn: Mutex::new(Some(conn)),
            readers: Mutex::new(Vec::new()),
            file: file.to_path_buf(),
            geo,
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Run `f` on the main connection.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> SqlResult<T>) -> SqlResult<T> {
        let g = self.conn.lock();
        match g.as_ref() {
            Some(c) => f(c),
            None => Err(closed_err()),
        }
    }

    /// Run `f` inside BEGIN … COMMIT (ROLLBACK on error).
    pub fn tx<T>(&self, f: impl FnOnce(&Connection) -> SqlResult<T>) -> SqlResult<T> {
        self.with(|c| {
            c.execute_batch("BEGIN")?;
            match f(c) {
                Ok(v) => {
                    c.execute_batch("COMMIT")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = c.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
    }

    /// Run `f` on a read-only connection (falls back to the main connection for `:memory:`).
    /// Meant for long reads, typically from a blocking thread.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> SqlResult<T>) -> SqlResult<T> {
        if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(closed_err());
        }
        if self.file.as_os_str() == ":memory:" {
            return self.with(f);
        }
        let conn = self.readers.lock().pop();
        let conn = match conn {
            Some(c) => c,
            None => {
                let c = Connection::open_with_flags(
                    &self.file,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
                )?;
                c.busy_timeout(std::time::Duration::from_secs(5))?;
                register_functions(&c, &self.geo)?;
                c.set_prepared_statement_cache_capacity(64);
                c
            }
        };
        let r = f(&conn);
        if !self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            let mut pool = self.readers.lock();
            if pool.len() < 4 {
                pool.push(conn);
            }
        }
        r
    }

    /// `run(sql, ...params)` → changes
    pub fn run(&self, sql: &str, params: Params) -> SqlResult<usize> {
        self.with(|c| {
            c.prepare_cached(sql)?
                .execute(rusqlite::params_from_iter(params))
        })
    }

    /// INSERT → last insert rowid
    pub fn insert(&self, sql: &str, params: Params) -> SqlResult<i64> {
        self.with(|c| {
            c.prepare_cached(sql)?
                .execute(rusqlite::params_from_iter(params))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// first row mapped
    pub fn get<T>(
        &self,
        sql: &str,
        params: Params,
        f: impl FnOnce(&Row) -> SqlResult<T>,
    ) -> SqlResult<Option<T>> {
        self.with(|c| query_one(c, sql, params, f))
    }

    /// all rows mapped
    pub fn all<T>(
        &self,
        sql: &str,
        params: Params,
        f: impl FnMut(&Row) -> SqlResult<T>,
    ) -> SqlResult<Vec<T>> {
        self.with(|c| query_all(c, sql, params, f))
    }

    /// single integer (count(*) etc.); None for no row / NULL
    pub fn int(&self, sql: &str, params: Params) -> SqlResult<Option<i64>> {
        Ok(self.get(sql, params, |r| Ok(num_i(r, 0)))?.flatten())
    }

    /// stored setting (JSON), None when missing / not JSON
    pub fn get_setting(&self, key: &str) -> Option<serde_json::Value> {
        let v = self
            .get("SELECT value FROM settings WHERE key = ?", p![key], |r| {
                r.get::<_, String>(0)
            })
            .ok()??;
        serde_json::from_str(&v).ok()
    }

    /// store a setting as JSON
    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> SqlResult<()> {
        self.run(
            "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            p![key, serde_json::to_string(value).unwrap_or_default()],
        )?;
        Ok(())
    }

    /// IANA zone used for UTC-only timestamps (stored settings → server zone).
    pub fn home_time_zone(&self) -> String {
        let tz = self.get_setting("settings").and_then(|s| {
            s.get("homeTimeZone")
                .and_then(|v| v.as_str().map(String::from))
        });
        match tz {
            Some(t) if pa_core::is_valid_time_zone(&t) => t,
            _ => system_time_zone(),
        }
    }

    /// close every connection (later calls fail with "database is not open")
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.readers.lock().clear();
        if let Some(c) = self.conn.lock().take() {
            let _ = c.close();
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// first row of a query on a connection
pub fn query_one<T>(
    c: &Connection,
    sql: &str,
    params: Params,
    f: impl FnOnce(&Row) -> SqlResult<T>,
) -> SqlResult<Option<T>> {
    let mut st = c.prepare_cached(sql)?;
    let mut rows = st.query(rusqlite::params_from_iter(params))?;
    match rows.next()? {
        Some(r) => Ok(Some(f(r)?)),
        None => Ok(None),
    }
}

/// all rows of a query on a connection
pub fn query_all<T>(
    c: &Connection,
    sql: &str,
    params: Params,
    mut f: impl FnMut(&Row) -> SqlResult<T>,
) -> SqlResult<Vec<T>> {
    let mut st = c.prepare_cached(sql)?;
    let mut rows = st.query(rusqlite::params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(f(r)?);
    }
    Ok(out)
}

/// column as a JS number → i64 (INTEGER or REAL; NULL / TEXT → None)
pub fn num_i<I: rusqlite::RowIndex>(r: &Row, i: I) -> Option<i64> {
    match r.get_ref(i).ok()? {
        ValueRef::Integer(v) => Some(v),
        ValueRef::Real(f) => Some(f as i64),
        _ => None,
    }
}

/// column as f64 (INTEGER or REAL)
pub fn num_f<I: rusqlite::RowIndex>(r: &Row, i: I) -> Option<f64> {
    match r.get_ref(i).ok()? {
        ValueRef::Integer(v) => Some(v as f64),
        ValueRef::Real(f) => Some(f),
        _ => None,
    }
}

/// column as text
pub fn text<I: rusqlite::RowIndex>(r: &Row, i: I) -> Option<String> {
    match r.get_ref(i).ok()? {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(v) => Some(v.to_string()),
        ValueRef::Real(f) => Some(pa_core::js::num_to_string(f)),
        _ => None,
    }
}

fn value_f64(v: ValueRef<'_>) -> Option<f64> {
    match v {
        ValueRef::Integer(i) => Some(i as f64),
        ValueRef::Real(f) => Some(f),
        _ => None,
    }
}

fn value_str(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => pa_core::js::num_to_string(f),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(_) => String::new(),
    }
}

fn value_opt_str(v: ValueRef<'_>) -> Option<String> {
    match v {
        ValueRef::Null => None,
        other => Some(value_str(other)),
    }
}

/// `systemTimeZone()`: the zone of this machine (TZ env first), UTC when unknown
pub fn system_time_zone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':').to_string();
        if pa_core::is_valid_time_zone(&tz) {
            return icu_canonical_zone(&tz).to_string();
        }
    }
    match iana_time_zone::get_timezone() {
        Ok(tz) if pa_core::is_valid_time_zone(&tz) => icu_canonical_zone(&tz).to_string(),
        _ => "UTC".into(),
    }
}

/// the id ICU (`Intl.DateTimeFormat().resolvedOptions().timeZone`) reports for a tzdb name: links
/// resolve to ICU's canonical ids ("Etc/UTC" → "UTC", "Asia/Kolkata" → "Asia/Calcutta")
pub fn icu_canonical_zone(tz: &str) -> &str {
    // generated with Node 22 over /usr/share/zoneinfo
    const LINKS: &[(&str, &str)] = &[
        ("Africa/Asmara", "Africa/Asmera"),
        ("Africa/Timbuktu", "Africa/Bamako"),
        ("America/Argentina/Buenos_Aires", "America/Buenos_Aires"),
        ("America/Argentina/Catamarca", "America/Catamarca"),
        ("America/Argentina/Cordoba", "America/Cordoba"),
        ("America/Argentina/Jujuy", "America/Jujuy"),
        ("America/Argentina/Mendoza", "America/Mendoza"),
        ("America/Atikokan", "America/Coral_Harbour"),
        ("America/Atka", "America/Adak"),
        ("America/Ensenada", "America/Tijuana"),
        ("America/Indiana/Indianapolis", "America/Indianapolis"),
        ("America/Kentucky/Louisville", "America/Louisville"),
        ("America/Montreal", "America/Toronto"),
        ("America/Nipigon", "America/Toronto"),
        ("America/Nuuk", "America/Godthab"),
        ("America/Pangnirtung", "America/Iqaluit"),
        ("America/Porto_Acre", "America/Rio_Branco"),
        ("America/Rainy_River", "America/Winnipeg"),
        ("America/Santa_Isabel", "America/Tijuana"),
        ("America/Shiprock", "America/Denver"),
        ("America/Thunder_Bay", "America/Toronto"),
        ("America/Virgin", "America/St_Thomas"),
        ("America/Yellowknife", "America/Edmonton"),
        ("Asia/Choibalsan", "Asia/Ulaanbaatar"),
        ("Asia/Chongqing", "Asia/Shanghai"),
        ("Asia/Harbin", "Asia/Shanghai"),
        ("Asia/Ho_Chi_Minh", "Asia/Saigon"),
        ("Asia/Istanbul", "Europe/Istanbul"),
        ("Asia/Kashgar", "Asia/Urumqi"),
        ("Asia/Kathmandu", "Asia/Katmandu"),
        ("Asia/Kolkata", "Asia/Calcutta"),
        ("Asia/Tel_Aviv", "Asia/Jerusalem"),
        ("Asia/Yangon", "Asia/Rangoon"),
        ("Atlantic/Faroe", "Atlantic/Faeroe"),
        ("Atlantic/Jan_Mayen", "Arctic/Longyearbyen"),
        ("Australia/Canberra", "Australia/Sydney"),
        ("Australia/Currie", "Australia/Hobart"),
        ("Australia/Yancowinna", "Australia/Broken_Hill"),
        ("CET", "Europe/Brussels"),
        ("EET", "Europe/Athens"),
        ("EST", "America/Panama"),
        ("Etc/GMT", "UTC"),
        ("Etc/GMT+0", "UTC"),
        ("Etc/GMT-0", "UTC"),
        ("Etc/GMT0", "UTC"),
        ("Etc/Greenwich", "UTC"),
        ("Etc/UCT", "UTC"),
        ("Etc/UTC", "UTC"),
        ("Etc/Universal", "UTC"),
        ("Etc/Zulu", "UTC"),
        ("Europe/Belfast", "Europe/London"),
        ("Europe/Kyiv", "Europe/Kiev"),
        ("Europe/Nicosia", "Asia/Nicosia"),
        ("Europe/Tiraspol", "Europe/Chisinau"),
        ("GMT", "UTC"),
        ("HST", "Pacific/Honolulu"),
        ("MET", "Europe/Brussels"),
        ("MST", "America/Phoenix"),
        ("Pacific/Chuuk", "Pacific/Truk"),
        ("Pacific/Johnston", "Pacific/Honolulu"),
        ("Pacific/Kanton", "Pacific/Enderbury"),
        ("Pacific/Pohnpei", "Pacific/Ponape"),
        ("Pacific/Samoa", "Pacific/Pago_Pago"),
        ("Pacific/Yap", "Pacific/Truk"),
        ("WET", "Europe/Lisbon"),
    ];
    LINKS
        .iter()
        .find(|(a, _)| *a == tz)
        .map(|(_, b)| *b)
        .unwrap_or(tz)
}

/// media.sun for rows whose sun position can't be known
pub const SUN_NA: f64 = 999.0;

/// sun altitude at capture (rounded to 0.1°), [`SUN_NA`] when unknown (`sunOf`)
pub fn sun_of(
    taken_at: Option<f64>,
    taken_local: Option<f64>,
    time_src: Option<f64>,
    lat: Option<f64>,
    lon: Option<f64>,
) -> f64 {
    let Some(taken_at) = taken_at else {
        return SUN_NA;
    };
    let (Some(la), Some(lo)) = (lat, lon) else {
        return SUN_NA;
    };
    if !pa_core::is_valid_gps(la, lo) {
        return SUN_NA;
    }
    let src = time_src.unwrap_or(f64::NAN);
    if src == pa_meta::time_src::MTIME as f64 {
        return SUN_NA;
    }
    let instant = match (src == pa_meta::time_src::LOCAL as f64, taken_local) {
        (true, Some(local)) => pa_core::estimate_instant(local as i64, taken_at as i64, lo) as f64,
        _ => taken_at,
    };
    let a = pa_core::sun_altitude(instant, la, lo);
    if a.is_finite() {
        pa_core::js::round(a * 10.0) / 10.0
    } else {
        SUN_NA
    }
}

fn register_functions(c: &Connection, geo: &Arc<GeoService>) -> SqlResult<()> {
    let det = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    c.create_scalar_function("peek_fold", 1, det, |ctx| {
        let v = ctx.get_raw(0);
        Ok(match v {
            ValueRef::Null => String::new(),
            other => pa_core::fold_text(Some(&value_str(other))),
        })
    })?;
    c.create_scalar_function("peek_local", 2, det, |ctx| {
        let instant = value_f64(ctx.get_raw(0));
        let tz = value_str(ctx.get_raw(1));
        let tz = if tz.is_empty() || tz == "0" {
            "UTC".to_string()
        } else {
            tz
        };
        Ok(instant.map(|i| pa_core::instant_to_local(i as i64, &tz)))
    })?;
    c.create_scalar_function("peek_instant", 2, det, |ctx| {
        let local = value_f64(ctx.get_raw(0));
        let tz = value_str(ctx.get_raw(1));
        let tz = if tz.is_empty() || tz == "0" {
            "UTC".to_string()
        } else {
            tz
        };
        Ok(local.map(|l| pa_core::local_to_instant(l as i64, &tz)))
    })?;
    let g = geo.clone();
    c.create_scalar_function("peek_search", 6, det, move |ctx| {
        let s = |i: usize| value_str(ctx.get_raw(i));
        let cc = value_opt_str(ctx.get_raw(2));
        let a1 = value_opt_str(ctx.get_raw(3));
        Ok(format!(
            "{} {} {} {}",
            pa_core::fold_text(Some(&s(0))),
            pa_core::fold_text(Some(&s(1))),
            g.place_text(cc.as_deref(), a1.as_deref()),
            pa_core::fold_text(Some(&format!("{} {}", s(4), s(5))))
        ))
    })?;
    c.create_scalar_function("peek_color", 1, det, |ctx| {
        let h = match ctx.get_raw(0) {
            ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
            _ => None,
        };
        Ok(pa_core::color_index_of_thumbhash(h.as_deref()) as i64)
    })?;
    c.create_scalar_function("peek_sun", 5, det, |ctx| {
        let f = |i: usize| value_f64(ctx.get_raw(i));
        let ts = match ctx.get_raw(2) {
            ValueRef::Null => Some(0.0),
            ValueRef::Text(t) => String::from_utf8_lossy(t).trim().parse::<f64>().ok(),
            other => value_f64(other),
        };
        Ok(sun_of(f(0), f(1), ts, f(3), f(4)))
    })?;
    Ok(())
}

// ───────────────────────── migrations ─────────────────────────

/// names of the migrations (PRAGMA user_version = number applied)
pub const MIGRATIONS: [&str; 5] = [
    "001-groups",
    "002-devices",
    "003-remote",
    "004-explore",
    "005-albums",
];

fn columns_of(c: &Connection, table: &str) -> SqlResult<std::collections::HashSet<String>> {
    let mut st = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = st.query_map([], |r| r.get::<_, String>("name"))?;
    rows.collect()
}

fn add_columns(c: &Connection, table: &str, cols: &[(&str, &str)]) -> SqlResult<()> {
    let have = columns_of(c, table)?;
    for (name, ty) in cols {
        if !have.contains(*name) {
            c.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {ty}"))?;
        }
    }
    Ok(())
}

/// `homeZoneFromSettings`
fn home_zone_from_settings(c: &Connection) -> String {
    let v: Option<String> = c
        .query_row(
            "SELECT value FROM settings WHERE key = 'settings'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(v) = v {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&v) {
            if let Some(tz) = j.get("homeTimeZone").and_then(|t| t.as_str()) {
                if pa_core::is_valid_time_zone(tz) {
                    return tz.to_string();
                }
            }
        }
    }
    system_time_zone()
}

fn m001(c: &Connection) -> SqlResult<()> {
    add_columns(
        c,
        "media",
        &[
            ("lat", "REAL"),
            ("lon", "REAL"),
            ("geo_id", "INTEGER"),
            ("cc", "TEXT"),
            ("admin1", "TEXT"),
            ("city", "TEXT"),
            ("make", "TEXT"),
            ("model", "TEXT"),
            ("lens", "TEXT"),
            ("flags", "INTEGER NOT NULL DEFAULT 0"),
            ("meta_v", "INTEGER NOT NULL DEFAULT 0"),
            ("taken_local", "INTEGER"),
            ("tz_off", "INTEGER"),
            ("time_src", "INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS media_local ON media (taken_local, id, kind, folder_id, width, height, city);
         CREATE INDEX IF NOT EXISTS media_place ON media (cc, admin1);
         CREATE INDEX IF NOT EXISTS media_model ON media (model);
         CREATE INDEX IF NOT EXISTS media_flags ON media (flags);
         CREATE INDEX IF NOT EXISTS media_metav ON media (meta_v);",
    )?;
    c.execute(
        "UPDATE media SET
           time_src = CASE WHEN taken_at != mtime AND meta_state = 1 THEN (CASE kind WHEN 'video' THEN 3 ELSE 1 END) ELSE 0 END,
           taken_local = peek_local(CASE WHEN taken_at > 0 THEN taken_at ELSE mtime END, ?)
         WHERE taken_local IS NULL",
        [home_zone_from_settings(c)],
    )?;
    Ok(())
}

fn m002(c: &Connection) -> SqlResult<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS devices (
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          model TEXT NOT NULL DEFAULT '',
          token_hash TEXT,
          paired INTEGER NOT NULL DEFAULT 0,
          caps TEXT,
          created_at INTEGER NOT NULL,
          last_seen INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    add_columns(
        c,
        "folders",
        &[
            ("device_id", "TEXT REFERENCES devices(id) ON DELETE CASCADE"),
            ("device_key", "TEXT"),
        ],
    )?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS folders_device ON folders (device_id, device_key);
         CREATE TABLE IF NOT EXISTS device_dirs (
           folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
           dir TEXT NOT NULL,
           PRIMARY KEY (folder_id, dir)
         );",
    )
}

fn m003(c: &Connection) -> SqlResult<()> {
    add_columns(
        c,
        "folders",
        &[("remote", "TEXT"), ("remote_status", "TEXT")],
    )
}

fn m004(c: &Connection) -> SqlResult<()> {
    add_columns(
        c,
        "media",
        &[
            ("color", "INTEGER"),
            ("sun", "REAL"),
            ("chash", "TEXT"),
            ("added_at", "INTEGER"),
        ],
    )?;
    c.execute_batch(
        "UPDATE media SET added_at = mtime WHERE added_at IS NULL;
      CREATE INDEX IF NOT EXISTS media_size ON media (size, kind, folder_id, chash);
      CREATE INDEX IF NOT EXISTS media_chash ON media (chash) WHERE chash IS NOT NULL;
      CREATE INDEX IF NOT EXISTS media_color ON media (color);
      CREATE INDEX IF NOT EXISTS media_added ON media (added_at);
      CREATE INDEX IF NOT EXISTS media_sun_todo ON media (id) WHERE sun IS NULL;
      CREATE INDEX IF NOT EXISTS media_color_todo ON media (id) WHERE color IS NULL AND thumbhash IS NOT NULL;

      CREATE TRIGGER IF NOT EXISTS media_added_at AFTER INSERT ON media WHEN NEW.added_at IS NULL
      BEGIN
        UPDATE media SET added_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_color_reset AFTER UPDATE OF thumbhash ON media
        WHEN NEW.thumbhash IS NOT OLD.thumbhash
      BEGIN
        UPDATE media SET color = NULL WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_sun_reset AFTER UPDATE OF taken_at, time_src, lat, lon ON media
        WHEN NEW.taken_at IS NOT OLD.taken_at OR NEW.time_src IS NOT OLD.time_src OR NEW.lat IS NOT OLD.lat OR NEW.lon IS NOT OLD.lon
      BEGIN
        UPDATE media SET sun = NULL WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_chash_reset AFTER UPDATE OF size, mtime ON media
        WHEN NEW.size IS NOT OLD.size OR NEW.mtime IS NOT OLD.mtime
      BEGIN
        UPDATE media SET chash = NULL WHERE id = NEW.id;
      END;",
    )
}

fn m005(c: &Connection) -> SqlResult<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS albums (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        cover_media_id INTEGER REFERENCES media(id) ON DELETE SET NULL,
        item_order TEXT NOT NULL DEFAULT 'desc',
        created INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        sort INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS album_items (
        album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
        media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
        added INTEGER NOT NULL,
        position REAL NOT NULL DEFAULT 0,
        PRIMARY KEY (album_id, media_id)
      ) WITHOUT ROWID;
      CREATE INDEX IF NOT EXISTS album_items_media ON album_items (media_id);
      CREATE INDEX IF NOT EXISTS album_items_pos ON album_items (album_id, position);",
    )?;
    add_columns(
        c,
        "libraries",
        &[
            (
                "cover_media_id",
                "INTEGER REFERENCES media(id) ON DELETE SET NULL",
            ),
            ("options", "TEXT"),
        ],
    )?;
    add_columns(c, "folders", &[("scanned_at", "INTEGER")])
}

type MigrationFn = fn(&Connection) -> SqlResult<()>;
const MIGRATION_FNS: [MigrationFn; 5] = [m001, m002, m003, m004, m005];

/// Apply pending migrations up to `upto` (tests build old schemas with fewer).
pub fn migrate_to(c: &Connection, file: &Path, memory: bool, upto: usize) -> SqlResult<()> {
    let current: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    let current = current.max(0) as usize;
    if current >= upto {
        return Ok(());
    }
    if !memory
        && std::fs::metadata(file)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let _ = std::fs::copy(file, format!("{}.bak-v{current}", file.display()));
    }
    for v in current..upto {
        c.execute_batch("BEGIN")?;
        let r = MIGRATION_FNS[v](c)
            .and_then(|_| c.execute_batch(&format!("PRAGMA user_version = {}", v + 1)));
        match r {
            Ok(()) => c.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = c.execute_batch("ROLLBACK");
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                    Some(format!("Migration {} failed: {e}", MIGRATIONS[v])),
                ));
            }
        }
    }
    Ok(())
}

fn migrate(c: &Connection, file: &Path, memory: bool) -> SqlResult<()> {
    migrate_to(c, file, memory, MIGRATIONS.len())
}

/// Open a database with only the first `upto` migrations (tests: old data dirs).
pub fn open_with_migrations(file: &Path, upto: usize) -> SqlResult<Connection> {
    let conn = Connection::open(file)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    register_functions(&conn, &Arc::new(GeoService::new(None)))?;
    migrate_to(&conn, file, false, upto)?;
    Ok(conn)
}

/// tiny error type for `Db::open`
pub mod anyhow_like {
    pub type Error = Box<dyn std::error::Error + Send + Sync>;
    pub type Result<T> = std::result::Result<T, Error>;
}

// ───────────────────────── rows ─────────────────────────

/// A `media` row (`SELECT *`).
#[derive(Debug, Clone, Default)]
pub struct MediaRow {
    pub id: i64,
    pub folder_id: i64,
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub kind: String,
    pub size: i64,
    pub mtime: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration: Option<f64>,
    pub taken_at: i64,
    pub thumbhash: Option<String>,
    pub favorite: i64,
    pub playback: String,
    pub meta_state: i64,
    pub thumb_state: i64,
    pub extra: Option<String>,
    pub taken_local: Option<i64>,
    pub tz_off: Option<i64>,
    pub time_src: i64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub geo_id: Option<i64>,
    pub cc: Option<String>,
    pub admin1: Option<String>,
    pub city: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub flags: i64,
    pub meta_v: i64,
}

fn col_i(r: &Row, n: &str) -> Option<i64> {
    num_i(r, n)
}
fn col_f(r: &Row, n: &str) -> Option<f64> {
    num_f(r, n)
}
fn col_s(r: &Row, n: &str) -> Option<String> {
    text(r, n)
}

impl MediaRow {
    /// from a `SELECT *` (or any superset of the item columns) row; missing columns stay default
    pub fn from_row(r: &Row) -> SqlResult<MediaRow> {
        Ok(MediaRow {
            id: col_i(r, "id").unwrap_or(0),
            folder_id: col_i(r, "folder_id").unwrap_or(0),
            dir: col_s(r, "dir").unwrap_or_default(),
            name: col_s(r, "name").unwrap_or_default(),
            ext: col_s(r, "ext").unwrap_or_default(),
            kind: col_s(r, "kind").unwrap_or_default(),
            size: col_i(r, "size").unwrap_or(0),
            mtime: col_i(r, "mtime").unwrap_or(0),
            width: col_i(r, "width"),
            height: col_i(r, "height"),
            duration: col_f(r, "duration"),
            taken_at: col_i(r, "taken_at").unwrap_or(0),
            thumbhash: col_s(r, "thumbhash"),
            favorite: col_i(r, "favorite").unwrap_or(0),
            playback: col_s(r, "playback").unwrap_or_else(|| "direct".into()),
            meta_state: col_i(r, "meta_state").unwrap_or(0),
            thumb_state: col_i(r, "thumb_state").unwrap_or(0),
            extra: col_s(r, "extra"),
            taken_local: col_i(r, "taken_local"),
            tz_off: col_i(r, "tz_off"),
            time_src: col_i(r, "time_src").unwrap_or(0),
            lat: col_f(r, "lat"),
            lon: col_f(r, "lon"),
            geo_id: col_i(r, "geo_id"),
            cc: col_s(r, "cc"),
            admin1: col_s(r, "admin1"),
            city: col_s(r, "city"),
            make: col_s(r, "make"),
            model: col_s(r, "model"),
            lens: col_s(r, "lens"),
            flags: col_i(r, "flags").unwrap_or(0),
            meta_v: col_i(r, "meta_v").unwrap_or(0),
        })
    }

    /// `toItem`
    pub fn to_item(&self) -> pa_core::MediaItem {
        let taken = if self.taken_at != 0 {
            self.taken_at
        } else {
            self.mtime
        };
        pa_core::MediaItem {
            id: self.id.to_string(),
            folder_id: self.folder_id.to_string(),
            dir: self.dir.clone(),
            name: self.name.clone(),
            ext: self.ext.clone(),
            kind: kind_of_str(&self.kind),
            size: self.size,
            mtime: self.mtime,
            width: self.width,
            height: self.height,
            duration: self.duration,
            taken_at: taken,
            local_at: self.taken_local.unwrap_or(taken),
            flags: self.flags as u32,
            thumbhash: self.thumbhash.clone(),
            favorite: self.favorite != 0,
            playback: playback_of_str(&self.playback),
        }
    }

    /// `kind === 'image'`
    pub fn is(&self, kind: &str) -> bool {
        self.kind == kind
    }
}

/// 'image' → MediaKind
pub fn kind_of_str(s: &str) -> pa_core::MediaKind {
    match s {
        "video" => pa_core::MediaKind::Video,
        "audio" => pa_core::MediaKind::Audio,
        "archive" => pa_core::MediaKind::Archive,
        _ => pa_core::MediaKind::Image,
    }
}

/// 'convert' → Playback
pub fn playback_of_str(s: &str) -> pa_core::Playback {
    match s {
        "convert" => pa_core::Playback::Convert,
        "transcode" => pa_core::Playback::Transcode,
        _ => pa_core::Playback::Direct,
    }
}

/// columns of `toItem` (`ITEM_COLS`)
pub const ITEM_COLS: &str =
    "id, folder_id, dir, name, ext, kind, size, mtime, width, height, duration, taken_at, taken_local, flags, thumbhash, favorite, playback";

/// A `folders` row.
#[derive(Debug, Clone, Default)]
pub struct FolderRow {
    pub id: i64,
    pub library_id: Option<i64>,
    pub path: String,
    pub name: String,
    pub temp: i64,
    pub expires_at: Option<i64>,
    pub source_name: Option<String>,
    pub sort: i64,
    pub device_id: Option<String>,
    pub device_key: Option<String>,
    pub remote: Option<String>,
    pub remote_status: Option<String>,
    pub scanned_at: Option<i64>,
}

impl FolderRow {
    /// from a `SELECT *` row
    pub fn from_row(r: &Row) -> SqlResult<FolderRow> {
        Ok(FolderRow {
            id: col_i(r, "id").unwrap_or(0),
            library_id: col_i(r, "library_id"),
            path: col_s(r, "path").unwrap_or_default(),
            name: col_s(r, "name").unwrap_or_default(),
            temp: col_i(r, "temp").unwrap_or(0),
            expires_at: col_i(r, "expires_at"),
            source_name: col_s(r, "source_name"),
            sort: col_i(r, "sort").unwrap_or(0),
            device_id: col_s(r, "device_id"),
            device_key: col_s(r, "device_key"),
            remote: col_s(r, "remote"),
            remote_status: col_s(r, "remote_status"),
            scanned_at: col_i(r, "scanned_at"),
        })
    }

    /// a linked phone's folder
    pub fn is_device(&self) -> bool {
        self.device_id.as_deref().is_some_and(|d| !d.is_empty())
    }

    /// a MEGA link shown without downloading
    pub fn is_remote(&self) -> bool {
        self.remote.as_deref().is_some_and(|d| !d.is_empty())
    }
}
