//! Tâche N5 : base SQLite des amis et de la file de demandes d'amis.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use core_lib::types::{Error, Friend, FriendRequest, Result, TrustLevel, VerifyCode};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

/// Version courante du schéma (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 1;

const BUSY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_NAME_CHARS: usize = 64;

/// Sens d'une demande : reçue (`in`) ou envoyée (`out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

fn db(e: rusqlite::Error) -> Error {
    Error::Other(format!("base des amis : {e}"))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn check_key(key: &str) -> Result<()> {
    let ok = key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "clé publique invalide (64 caractères hexadécimaux minuscules attendus) : {key:?}"
        )))
    }
}

fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Invalid("nom vide".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Invalid(format!("nom trop long (max {MAX_NAME_CHARS} caractères)")));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::Invalid("nom : caractères de contrôle interdits".into()));
    }
    Ok(())
}

/// Format `XXXX · XXXX · XXXX · XXXX` (hex majuscules).
fn check_verify_code(code: &str) -> Result<()> {
    let groups: Vec<&str> = code.split(" · ").collect();
    let ok = groups.len() == 4
        && groups
            .iter()
            .all(|g| g.len() == 4 && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F')));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "code de vérification invalide (format « XXXX · XXXX · XXXX · XXXX » attendu) : {code:?}"
        )))
    }
}

fn trust_str(t: &TrustLevel) -> &'static str {
    match t {
        TrustLevel::Ask => "ask",
        TrustLevel::Always => "always",
    }
}

fn parse_trust(s: &str) -> rusqlite::Result<TrustLevel> {
    match s {
        "ask" => Ok(TrustLevel::Ask),
        "always" => Ok(TrustLevel::Always),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("niveau de confiance inconnu : {other}").into(),
        )),
    }
}

fn row_to_friend(r: &Row<'_>) -> rusqlite::Result<Friend> {
    let trust: String = r.get(2)?;
    Ok(Friend { public_key: r.get(0)?, name: r.get(1)?, trust: parse_trust(&trust)?, added_at: r.get(3)? })
}

fn row_to_request(r: &Row<'_>) -> rusqlite::Result<FriendRequest> {
    Ok(FriendRequest {
        public_key: r.get(0)?,
        name: r.get(1)?,
        verify_code: VerifyCode(r.get(2)?),
        received_at: r.get(3)?,
    })
}

/// Échappe `%`, `_` et `\` pour un `LIKE ... ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Migrations incrémentales : l'entrée `i` fait passer le schéma de la version `i` à `i + 1`.
const MIGRATIONS: &[&str] = &[
    // v0 -> v1
    "CREATE TABLE friends (
        public_key TEXT PRIMARY KEY NOT NULL,
        name       TEXT NOT NULL,
        trust      TEXT NOT NULL CHECK (trust IN ('ask','always')),
        added_at   INTEGER NOT NULL
     );
     CREATE TABLE requests (
        public_key  TEXT PRIMARY KEY NOT NULL,
        name        TEXT NOT NULL,
        verify_code TEXT NOT NULL,
        received_at INTEGER NOT NULL,
        direction   TEXT NOT NULL CHECK (direction IN ('in','out'))
     );
     CREATE INDEX idx_requests_received ON requests(received_at DESC);",
];

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(db)?;
    if current > SCHEMA_VERSION {
        return Err(Error::Other(format!(
            "base des amis trop récente (version {current}, supportée {SCHEMA_VERSION})"
        )));
    }
    for v in current..SCHEMA_VERSION {
        let tx = conn.transaction().map_err(db)?;
        tx.execute_batch(MIGRATIONS[v as usize]).map_err(db)?;
        tx.pragma_update(None, "user_version", v + 1).map_err(db)?;
        tx.commit().map_err(db)?;
    }
    Ok(())
}

/// Base SQLite des amis et des demandes d'amis.
pub struct FriendsDb {
    conn: Mutex<Connection>,
}

impl FriendsDb {
    /// Ouvre (et crée / migre) la base. Mode WAL.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref()).map_err(db)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(db)?;
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(db)?;
        Self::init(conn)
    }

    /// Base en mémoire (tests, mode éphémère).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Version de schéma de la base.
    pub fn schema_version(&self) -> Result<i64> {
        self.lock().query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(db)
    }

    // ----- amis -----

    /// Ajoute un ami. `Error::Invalid` si la clé est déjà présente ou si une entrée est invalide.
    pub fn add_friend(&self, friend: &Friend) -> Result<()> {
        check_key(&friend.public_key)?;
        check_name(&friend.name)?;
        insert_friend(&self.lock(), friend)
    }

    pub fn get_friend(&self, public_key: &str) -> Result<Option<Friend>> {
        check_key(public_key)?;
        self.lock()
            .query_row(
                "SELECT public_key, name, trust, added_at FROM friends WHERE public_key = ?1",
                params![public_key],
                row_to_friend,
            )
            .optional()
            .map_err(db)
    }

    pub fn is_friend(&self, public_key: &str) -> Result<bool> {
        check_key(public_key)?;
        self.lock()
            .query_row("SELECT 1 FROM friends WHERE public_key = ?1", params![public_key], |_| Ok(()))
            .optional()
            .map(|o| o.is_some())
            .map_err(db)
    }

    /// Tous les amis, triés par nom (insensible à la casse) puis par clé.
    pub fn list_friends(&self) -> Result<Vec<Friend>> {
        let conn = self.lock();
        let mut st = conn
            .prepare(
                "SELECT public_key, name, trust, added_at FROM friends
                 ORDER BY name COLLATE NOCASE, public_key",
            )
            .map_err(db)?;
        let rows = st.query_map([], row_to_friend).map_err(db)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
    }

    /// Supprime un ami ; renvoie `true` s'il existait.
    pub fn remove_friend(&self, public_key: &str) -> Result<bool> {
        check_key(public_key)?;
        let n = self
            .lock()
            .execute("DELETE FROM friends WHERE public_key = ?1", params![public_key])
            .map_err(db)?;
        Ok(n > 0)
    }

    /// Change le niveau de confiance. `NotFound` si l'ami est absent.
    pub fn set_trust(&self, public_key: &str, trust: TrustLevel) -> Result<()> {
        check_key(public_key)?;
        let n = self
            .lock()
            .execute(
                "UPDATE friends SET trust = ?2 WHERE public_key = ?1",
                params![public_key, trust_str(&trust)],
            )
            .map_err(db)?;
        if n == 0 {
            return Err(Error::NotFound(format!("ami {public_key}")));
        }
        Ok(())
    }

    /// Renomme un ami. `NotFound` si absent.
    pub fn rename(&self, public_key: &str, name: &str) -> Result<()> {
        check_key(public_key)?;
        check_name(name)?;
        let n = self
            .lock()
            .execute("UPDATE friends SET name = ?2 WHERE public_key = ?1", params![public_key, name])
            .map_err(db)?;
        if n == 0 {
            return Err(Error::NotFound(format!("ami {public_key}")));
        }
        Ok(())
    }

    // ----- demandes -----

    /// Insère ou remplace (par clé publique) une demande.
    pub fn upsert_request(&self, req: &FriendRequest, direction: Direction) -> Result<()> {
        check_key(&req.public_key)?;
        check_name(&req.name)?;
        check_verify_code(&req.verify_code.0)?;
        self.lock()
            .execute(
                "INSERT INTO requests (public_key, name, verify_code, received_at, direction)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(public_key) DO UPDATE SET
                    name = excluded.name, verify_code = excluded.verify_code,
                    received_at = excluded.received_at, direction = excluded.direction",
                params![req.public_key, req.name, req.verify_code.0, req.received_at, direction.as_str()],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn request_count(&self) -> Result<usize> {
        let n: i64 = self.lock().query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).map_err(db)?;
        Ok(n.max(0) as usize)
    }

    /// Page de demandes, de la plus récente à la plus ancienne. `query` : sous-chaîne du nom,
    /// insensible à la casse (ASCII), `%` et `_` traités littéralement.
    pub fn list_requests(&self, offset: usize, limit: usize, query: Option<&str>) -> Result<Vec<FriendRequest>> {
        let offset = i64::try_from(offset).map_err(|_| Error::Invalid("offset trop grand".into()))?;
        let limit = i64::try_from(limit).map_err(|_| Error::Invalid("limite trop grande".into()))?;
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{}%", like_escape(q)));
        let conn = self.lock();
        let mut st = conn
            .prepare(
                "SELECT public_key, name, verify_code, received_at FROM requests
                 WHERE (?1 IS NULL OR name LIKE ?1 ESCAPE '\\')
                 ORDER BY received_at DESC, public_key
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(db)?;
        let rows = st.query_map(params![pattern, limit, offset], row_to_request).map_err(db)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
    }

    /// Supprime une demande ; renvoie `true` si elle existait.
    pub fn remove_request(&self, public_key: &str) -> Result<bool> {
        check_key(public_key)?;
        let n = self
            .lock()
            .execute("DELETE FROM requests WHERE public_key = ?1", params![public_key])
            .map_err(db)?;
        Ok(n > 0)
    }

    /// Supprime toutes les demandes (« tout refuser ») ; renvoie leur nombre.
    pub fn remove_all_requests(&self) -> Result<usize> {
        self.lock().execute("DELETE FROM requests", []).map_err(db)
    }

    /// Accepte une demande, atomiquement : la demande est supprimée et l'ami créé (confiance
    /// `Ask`). `NotFound` si la demande est absente, `Invalid` si la clé est déjà ami
    /// (dans ce cas rien n'est modifié).
    pub fn accept_request(&self, public_key: &str) -> Result<Friend> {
        check_key(public_key)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db)?;
        let name: Option<String> = tx
            .query_row("SELECT name FROM requests WHERE public_key = ?1", params![public_key], |r| r.get(0))
            .optional()
            .map_err(db)?;
        let name = name.ok_or_else(|| Error::NotFound(format!("demande {public_key}")))?;
        tx.execute("DELETE FROM requests WHERE public_key = ?1", params![public_key]).map_err(db)?;
        let friend = Friend { public_key: public_key.to_string(), name, trust: TrustLevel::Ask, added_at: now() };
        insert_friend(&tx, &friend)?;
        tx.commit().map_err(db)?;
        Ok(friend)
    }
}

fn insert_friend(conn: &Connection, f: &Friend) -> Result<()> {
    match conn.execute(
        "INSERT INTO friends (public_key, name, trust, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![f.public_key, f.name, trust_str(&f.trust), f.added_at],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => {
            Err(Error::Invalid(format!("cet ami existe déjà : {}", f.public_key)))
        }
        Err(e) => Err(db(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u32) -> String {
        format!("{n:064x}")
    }

    fn friend(n: u32, name: &str) -> Friend {
        Friend { public_key: key(n), name: name.into(), trust: TrustLevel::Ask, added_at: 100 }
    }

    fn req(n: u32, name: &str, at: i64) -> FriendRequest {
        FriendRequest {
            public_key: key(n),
            name: name.into(),
            verify_code: VerifyCode("3F9A · C21B · 77D0 · 5E14".into()),
            received_at: at,
        }
    }

    #[test]
    fn friends_crud() {
        let d = FriendsDb::open_in_memory().unwrap();
        d.add_friend(&friend(1, "Zed")).unwrap();
        d.add_friend(&friend(2, "alice")).unwrap();
        assert!(d.is_friend(&key(1)).unwrap());
        assert!(!d.is_friend(&key(3)).unwrap());
        assert_eq!(d.get_friend(&key(2)).unwrap().unwrap().name, "alice");
        assert_eq!(d.get_friend(&key(3)).unwrap(), None);
        let names: Vec<_> = d.list_friends().unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["alice", "Zed"]);
        d.set_trust(&key(1), TrustLevel::Always).unwrap();
        assert_eq!(d.get_friend(&key(1)).unwrap().unwrap().trust, TrustLevel::Always);
        d.rename(&key(1), "Zedd").unwrap();
        assert_eq!(d.get_friend(&key(1)).unwrap().unwrap().name, "Zedd");
        assert!(matches!(d.set_trust(&key(9), TrustLevel::Ask), Err(Error::NotFound(_))));
        assert!(matches!(d.rename(&key(9), "x"), Err(Error::NotFound(_))));
        assert!(d.remove_friend(&key(1)).unwrap());
        assert!(!d.remove_friend(&key(1)).unwrap());
    }

    #[test]
    fn duplicate_friend_is_invalid() {
        let d = FriendsDb::open_in_memory().unwrap();
        d.add_friend(&friend(1, "A")).unwrap();
        match d.add_friend(&friend(1, "B")) {
            Err(Error::Invalid(m)) => assert!(m.contains("existe déjà")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validation() {
        let d = FriendsDb::open_in_memory().unwrap();
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(65)] {
            let mut f = friend(1, "x");
            f.public_key = bad.to_string();
            assert!(matches!(d.add_friend(&f), Err(Error::Invalid(_))), "{bad}");
            assert!(matches!(d.get_friend(bad), Err(Error::Invalid(_))));
        }
        for bad in ["", "   ", "a\nb", "a\u{0}b", &"é".repeat(65)] {
            assert!(matches!(d.add_friend(&friend(1, bad)), Err(Error::Invalid(_))), "{bad:?}");
        }
        d.add_friend(&friend(1, &"é".repeat(64))).unwrap();
        for bad in [
            "",
            "3f9a · c21b · 77d0 · 5e14",
            "3F9A-C21B-77D0-5E14",
            "3F9A · C21B · 77D0",
            "3F9A · C21B · 77D0 · 5E1G",
        ] {
            let mut r = req(2, "x", 1);
            r.verify_code = VerifyCode(bad.into());
            assert!(matches!(d.upsert_request(&r, Direction::In), Err(Error::Invalid(_))), "{bad}");
        }
        assert_eq!(d.request_count().unwrap(), 0);
    }

    #[test]
    fn migration_v0_to_v1() {
        let mut conn = Connection::open_in_memory().unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 0);
        migrate(&mut conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // Idempotent.
        migrate(&mut conn).unwrap();
        for t in ["friends", "requests"] {
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1", [t], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "{t}");
        }
        // Version future refusée.
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    #[test]
    fn many_requests_paginate_and_search() {
        let d = FriendsDb::open_in_memory().unwrap();
        let t0 = std::time::Instant::now();
        for i in 0..1000u32 {
            d.upsert_request(&req(i, &format!("Player{i}"), i64::from(i)), Direction::In).unwrap();
        }
        assert_eq!(d.request_count().unwrap(), 1000);
        let p1 = d.list_requests(0, 50, None).unwrap();
        assert_eq!(p1.len(), 50);
        assert_eq!(p1[0].name, "Player999");
        assert!(p1.windows(2).all(|w| w[0].received_at > w[1].received_at));
        let p2 = d.list_requests(50, 50, None).unwrap();
        assert_eq!(p2[0].name, "Player949");
        assert_eq!(d.list_requests(990, 50, None).unwrap().len(), 10);
        assert!(d.list_requests(1000, 50, None).unwrap().is_empty());
        let found = d.list_requests(0, 2000, Some("PLAYER99")).unwrap();
        assert_eq!(found.len(), 11); // 99, 990..=999
        assert_eq!(d.list_requests(0, 10, Some("   ")).unwrap().len(), 10);
        assert!(t0.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn upsert_replaces() {
        let d = FriendsDb::open_in_memory().unwrap();
        d.upsert_request(&req(1, "Old", 1), Direction::In).unwrap();
        d.upsert_request(&req(1, "New", 5), Direction::Out).unwrap();
        assert_eq!(d.request_count().unwrap(), 1);
        let l = d.list_requests(0, 10, None).unwrap();
        assert_eq!((l[0].name.as_str(), l[0].received_at), ("New", 5));
    }

    #[test]
    fn refuse_all() {
        let d = FriendsDb::open_in_memory().unwrap();
        for i in 0..10 {
            d.upsert_request(&req(i, "n", 1), Direction::In).unwrap();
        }
        assert!(d.remove_request(&key(0)).unwrap());
        assert!(!d.remove_request(&key(0)).unwrap());
        assert_eq!(d.remove_all_requests().unwrap(), 9);
        assert_eq!(d.remove_all_requests().unwrap(), 0);
        assert_eq!(d.request_count().unwrap(), 0);
    }

    #[test]
    fn accept_is_atomic() {
        let d = FriendsDb::open_in_memory().unwrap();
        d.upsert_request(&req(1, "Bob", 1), Direction::In).unwrap();
        let f = d.accept_request(&key(1)).unwrap();
        assert_eq!((f.name.as_str(), f.trust.clone()), ("Bob", TrustLevel::Ask));
        assert!(d.is_friend(&key(1)).unwrap());
        assert_eq!(d.request_count().unwrap(), 0);
        assert!(matches!(d.accept_request(&key(1)), Err(Error::NotFound(_))));
        // Déjà ami : erreur, et la demande est conservée (rollback).
        d.upsert_request(&req(1, "Bob2", 2), Direction::In).unwrap();
        assert!(matches!(d.accept_request(&key(1)), Err(Error::Invalid(_))));
        assert_eq!(d.request_count().unwrap(), 1);
        assert_eq!(d.get_friend(&key(1)).unwrap().unwrap().name, "Bob");
    }

    #[test]
    fn sql_injection_is_inert() {
        let d = FriendsDb::open_in_memory().unwrap();
        let evil = "'; DROP TABLE friends;--";
        d.add_friend(&friend(1, evil)).unwrap();
        d.upsert_request(&req(2, evil, 1), Direction::In).unwrap();
        assert_eq!(d.list_requests(0, 10, Some(evil)).unwrap().len(), 1);
        assert!(d.list_requests(0, 10, Some("' OR '1'='1")).unwrap().is_empty());
        assert_eq!(d.get_friend(&key(1)).unwrap().unwrap().name, evil);
        assert_eq!(d.list_friends().unwrap().len(), 1);
        d.rename(&key(1), evil).unwrap();
    }

    #[test]
    fn like_wildcards_are_literal() {
        let d = FriendsDb::open_in_memory().unwrap();
        d.upsert_request(&req(1, "100%_real", 1), Direction::In).unwrap();
        d.upsert_request(&req(2, "100abcreal", 2), Direction::In).unwrap();
        d.upsert_request(&req(3, "back\\slash", 3), Direction::In).unwrap();
        assert_eq!(d.list_requests(0, 10, Some("%")).unwrap().len(), 1);
        assert_eq!(d.list_requests(0, 10, Some("_")).unwrap().len(), 1);
        assert_eq!(d.list_requests(0, 10, Some("0%_r")).unwrap().len(), 1);
        assert_eq!(d.list_requests(0, 10, Some("\\")).unwrap().len(), 1);
        assert_eq!(d.list_requests(0, 10, Some("100")).unwrap().len(), 2);
    }

    #[test]
    fn persists_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("friends.db");
        {
            let d = FriendsDb::open(&path).unwrap();
            d.add_friend(&friend(1, "A")).unwrap();
            d.set_trust(&key(1), TrustLevel::Always).unwrap();
            d.upsert_request(&req(2, "B", 7), Direction::Out).unwrap();
        }
        let d = FriendsDb::open(&path).unwrap();
        assert_eq!(d.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(d.get_friend(&key(1)).unwrap().unwrap().trust, TrustLevel::Always);
        assert_eq!(d.list_requests(0, 10, None).unwrap()[0].received_at, 7);
    }
}
