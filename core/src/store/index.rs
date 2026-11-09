//! Tâche S2 : index SQLite du magasin adressé par contenu (SHA-256 -> chemin, taille, références).

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use crate::types::{Error, Result};

/// Version courante du schéma (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 2;

const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Une entrée de l'index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub sha256: String,
    /// Chemin relatif dans le magasin, avec `/`.
    pub rel_path: String,
    pub size: u64,
    /// Compteur de références (jamais négatif).
    pub refs: u32,
    /// Secondes Unix.
    pub created_at: i64,
    pub last_used: i64,
}

fn db(e: rusqlite::Error) -> Error {
    Error::Other(format!("index du magasin : {e}"))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn check_hash(sha256: &str) -> Result<()> {
    let ok = sha256.len() == 64 && sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("hash SHA-256 invalide : {sha256:?}")))
    }
}

/// Chemin relatif d'un objet dans le magasin : `ab/cd/abcdef…`.
pub fn object_rel_path(sha256: &str) -> Result<String> {
    check_hash(sha256)?;
    Ok(format!("{}/{}/{}", &sha256[0..2], &sha256[2..4], sha256))
}

const COLS: &str = "sha256, rel_path, size, refs, created_at, last_used";

fn row_to_entry(r: &Row<'_>) -> rusqlite::Result<Entry> {
    let size: i64 = r.get(2)?;
    let refs: i64 = r.get(3)?;
    Ok(Entry {
        sha256: r.get(0)?,
        rel_path: r.get(1)?,
        size: size.max(0) as u64,
        refs: refs.max(0) as u32,
        created_at: r.get(4)?,
        last_used: r.get(5)?,
    })
}

/// Index du magasin. Une instance = une connexion ; on peut en ouvrir plusieurs sur le même fichier.
pub struct StoreIndex {
    conn: Mutex<Connection>,
}

impl StoreIndex {
    /// Ouvre (et crée / migre) l'index. Mode WAL, `busy_timeout` de 30 s.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut conn = Connection::open(path.as_ref()).map_err(db)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        // Le passage en WAL peut échouer immédiatement en SQLITE_BUSY (le busy_timeout ne s'y applique pas)
        // quand plusieurs connexions ouvrent un fichier neuf en même temps : on réessaie brièvement.
        let mut tries = 0;
        loop {
            match conn.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => break,
                Err(_) if tries < 200 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => return Err(db(e)),
            }
        }
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(db)?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Version de schéma de la base.
    pub fn schema_version(&self) -> Result<i64> {
        self.lock().query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(db)
    }

    /// Enregistre un objet. Idempotent : si le hash existe déjà, seules les dates sont rafraîchies
    /// (chemin, taille et références sont conservés).
    pub fn insert(&self, sha256: &str, rel_path: &str, size: u64) -> Result<()> {
        check_hash(sha256)?;
        let size = i64::try_from(size).map_err(|_| Error::Invalid("taille trop grande".into()))?;
        let t = now();
        self.lock()
            .execute(
                "INSERT INTO objects (sha256, rel_path, size, refs, created_at, last_used)
                 VALUES (?1, ?2, ?3, 0, ?4, ?4)
                 ON CONFLICT(sha256) DO UPDATE SET last_used = excluded.last_used",
                params![sha256, rel_path, size, t],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn get(&self, sha256: &str) -> Result<Option<Entry>> {
        check_hash(sha256)?;
        self.lock()
            .query_row(
                &format!("SELECT {COLS} FROM objects WHERE sha256 = ?1"),
                params![sha256],
                row_to_entry,
            )
            .optional()
            .map_err(db)
    }

    pub fn contains(&self, sha256: &str) -> Result<bool> {
        check_hash(sha256)?;
        self.lock()
            .query_row("SELECT 1 FROM objects WHERE sha256 = ?1", params![sha256], |_| Ok(()))
            .optional()
            .map(|o| o.is_some())
            .map_err(db)
    }

    /// Incrémente le compteur de références ; renvoie la nouvelle valeur. `NotFound` si absent.
    pub fn add_ref(&self, sha256: &str) -> Result<u32> {
        self.bump(sha256, "refs + 1")
    }

    /// Décrémente le compteur (plancher à 0) ; renvoie la nouvelle valeur. `NotFound` si absent.
    pub fn release_ref(&self, sha256: &str) -> Result<u32> {
        self.bump(sha256, "MAX(refs - 1, 0)")
    }

    fn bump(&self, sha256: &str, expr: &str) -> Result<u32> {
        check_hash(sha256)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db)?;
        let n = tx
            .execute(
                &format!("UPDATE objects SET refs = {expr}, last_used = ?2 WHERE sha256 = ?1"),
                params![sha256, now()],
            )
            .map_err(db)?;
        if n == 0 {
            return Err(Error::NotFound(format!("objet {sha256}")));
        }
        let refs: i64 = tx
            .query_row("SELECT refs FROM objects WHERE sha256 = ?1", params![sha256], |r| r.get(0))
            .map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(refs.max(0) as u32)
    }

    /// Objets sans référence (candidats au GC), du plus ancien au plus récent.
    pub fn unreferenced(&self) -> Result<Vec<Entry>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {COLS} FROM objects WHERE refs = 0 ORDER BY last_used, sha256"
            ))
            .map_err(db)?;
        let rows = stmt.query_map([], row_to_entry).map_err(db)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
    }

    /// Supprime l'entrée ; renvoie vrai si elle existait.
    pub fn remove(&self, sha256: &str) -> Result<bool> {
        check_hash(sha256)?;
        let n = self
            .lock()
            .execute("DELETE FROM objects WHERE sha256 = ?1", params![sha256])
            .map_err(db)?;
        Ok(n > 0)
    }

    /// Somme des tailles de tous les objets indexés.
    pub fn total_size(&self) -> Result<u64> {
        let total: i64 = self
            .lock()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM objects", [], |r| r.get(0))
            .map_err(db)?;
        Ok(total.max(0) as u64)
    }
}

/// Migrations transactionnelles (verrou `IMMEDIATE`, version relue sous verrou : sûr si plusieurs
/// connexions s'ouvrent en même temps ; un crash en cours de migration annule tout).
fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db)?;
    let mut version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(db)?;
    if version > SCHEMA_VERSION {
        return Err(Error::Other(format!(
            "index du magasin : schéma v{version} plus récent que v{SCHEMA_VERSION}"
        )));
    }
    if version < 1 {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS objects (
                sha256   TEXT PRIMARY KEY NOT NULL,
                rel_path TEXT NOT NULL,
                size     INTEGER NOT NULL
            );",
        )
        .map_err(db)?;
        version = 1;
    }
    if version < 2 {
        let has = |col: &str| -> rusqlite::Result<bool> {
            tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('objects') WHERE name = ?1",
                params![col],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        };
        if !has("refs").map_err(db)? {
            tx.execute_batch("ALTER TABLE objects ADD COLUMN refs INTEGER NOT NULL DEFAULT 0;")
                .map_err(db)?;
        }
        for col in ["created_at", "last_used"] {
            if !has(col).map_err(db)? {
                tx.execute_batch(&format!(
                    "ALTER TABLE objects ADD COLUMN {col} INTEGER NOT NULL DEFAULT 0;"
                ))
                .map_err(db)?;
            }
        }
        tx.execute_batch("CREATE INDEX IF NOT EXISTS idx_objects_refs ON objects(refs);")
            .map_err(db)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION).map_err(db)?;
    tx.commit().map_err(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn h(n: u32) -> String {
        format!("{n:064x}")
    }

    fn open_tmp() -> (tempfile::TempDir, std::path::PathBuf, StoreIndex) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("index.db");
        let idx = StoreIndex::open(&p).unwrap();
        (dir, p, idx)
    }

    #[test]
    fn crud_and_total() {
        let (_d, _p, idx) = open_tmp();
        let a = h(0xabcdef);
        assert!(!idx.contains(&a).unwrap());
        assert_eq!(idx.get(&a).unwrap(), None);
        let rel = object_rel_path(&a).unwrap();
        idx.insert(&a, &rel, 100).unwrap();
        idx.insert(&h(2), "x/y/z", 50).unwrap();
        let e = idx.get(&a).unwrap().unwrap();
        assert_eq!((e.rel_path.as_str(), e.size, e.refs), (rel.as_str(), 100, 0));
        assert!(idx.contains(&a).unwrap());
        assert_eq!(idx.total_size().unwrap(), 150);
        assert!(idx.remove(&a).unwrap());
        assert!(!idx.remove(&a).unwrap());
        assert_eq!(idx.total_size().unwrap(), 50);
    }

    #[test]
    fn insert_is_idempotent() {
        let (_d, _p, idx) = open_tmp();
        let a = h(1);
        idx.insert(&a, "p1", 10).unwrap();
        idx.add_ref(&a).unwrap();
        idx.insert(&a, "autre", 99).unwrap();
        let e = idx.get(&a).unwrap().unwrap();
        assert_eq!((e.rel_path.as_str(), e.size, e.refs), ("p1", 10, 1));
        assert_eq!(idx.total_size().unwrap(), 10);
    }

    #[test]
    fn ref_counts_and_unreferenced() {
        let (_d, _p, idx) = open_tmp();
        let (a, b) = (h(1), h(2));
        idx.insert(&a, "a", 1).unwrap();
        idx.insert(&b, "b", 1).unwrap();
        assert_eq!(idx.add_ref(&a).unwrap(), 1);
        assert_eq!(idx.add_ref(&a).unwrap(), 2);
        let un = idx.unreferenced().unwrap();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].sha256, b);
        assert_eq!(idx.release_ref(&a).unwrap(), 1);
        assert_eq!(idx.release_ref(&a).unwrap(), 0);
        assert_eq!(idx.release_ref(&a).unwrap(), 0, "plancher à 0");
        assert_eq!(idx.unreferenced().unwrap().len(), 2);
        assert!(matches!(idx.add_ref(&h(9)), Err(Error::NotFound(_))));
        assert!(matches!(idx.release_ref(&h(9)), Err(Error::NotFound(_))));
    }

    #[test]
    fn concurrent_inserts_separate_connections() {
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(dir.path().join("index.db"));
        let handles: Vec<_> = (0..8u32)
            .map(|t| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let idx = StoreIndex::open(&*p).unwrap();
                    for i in 0..200u32 {
                        idx.insert(&h(t * 1000 + i), "p", 1).unwrap();
                        // chevauchement volontaire entre threads
                        idx.insert(&h(900_000 + i), "shared", 1).unwrap();
                        idx.add_ref(&h(900_000 + i)).unwrap();
                    }
                })
            })
            .collect();
        for j in handles {
            j.join().unwrap();
        }
        let idx = StoreIndex::open(&*p).unwrap();
        assert_eq!(idx.total_size().unwrap(), 8 * 200 + 200);
        assert_eq!(idx.get(&h(900_000)).unwrap().unwrap().refs, 8);
    }

    #[test]
    fn shared_instance_across_threads() {
        let (_d, _p, idx) = open_tmp();
        let idx = Arc::new(idx);
        let hs: Vec<_> = (0..4u32)
            .map(|t| {
                let idx = idx.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        idx.insert(&h(t * 100 + i), "p", 2).unwrap();
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|j| j.join().unwrap());
        assert_eq!(idx.total_size().unwrap(), 400);
    }

    #[test]
    fn reopen_persists() {
        let (_d, p, idx) = open_tmp();
        idx.insert(&h(5), "r", 7).unwrap();
        idx.add_ref(&h(5)).unwrap();
        drop(idx);
        let idx = StoreIndex::open(&p).unwrap();
        let e = idx.get(&h(5)).unwrap().unwrap();
        assert_eq!((e.size, e.refs), (7, 1));
        assert_eq!(idx.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrates_old_schema() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("old.db");
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch(
                "CREATE TABLE objects (sha256 TEXT PRIMARY KEY NOT NULL, rel_path TEXT NOT NULL, size INTEGER NOT NULL);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
            c.execute("INSERT INTO objects VALUES (?1, 'old/path', 42)", params![h(7)]).unwrap();
        }
        let idx = StoreIndex::open(&p).unwrap();
        assert_eq!(idx.schema_version().unwrap(), SCHEMA_VERSION);
        let e = idx.get(&h(7)).unwrap().unwrap();
        assert_eq!((e.rel_path.as_str(), e.size, e.refs), ("old/path", 42, 0));
        assert_eq!(idx.add_ref(&h(7)).unwrap(), 1);
    }

    #[test]
    fn rejects_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("new.db");
        Connection::open(&p).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(StoreIndex::open(&p).is_err());
    }

    #[test]
    fn invalid_hash_rejected() {
        let (_d, _p, idx) = open_tmp();
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
            assert!(matches!(object_rel_path(bad), Err(Error::Invalid(_))), "{bad}");
            assert!(matches!(idx.insert(bad, "p", 1), Err(Error::Invalid(_))));
            assert!(matches!(idx.get(bad), Err(Error::Invalid(_))));
            assert!(matches!(idx.add_ref(bad), Err(Error::Invalid(_))));
        }
    }

    #[test]
    fn rel_path_layout() {
        let hash = format!("abcd{}", "0".repeat(60));
        assert_eq!(object_rel_path(&hash).unwrap(), format!("ab/cd/{hash}"));
    }
}
