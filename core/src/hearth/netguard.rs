//! Exposition réseau du serveur dédié (voir `docs/hearth.md`, § 6.7).
//!
//! Le serveur d'un groupe ne doit écouter que sur la boucle locale : les amis arrivent par le tunnel P2P, et rien d'autre
//! ne doit être joignable depuis le réseau de l'hôte. Deux protections :
//!
//! * **avant le démarrage** : [`harden_configs`] réécrit les configurations connues des mods qui ouvrent un port web
//!   (Dynmap, BlueMap, Squaremap) ou UDP (Simple Voice Chat) pour les lier à `127.0.0.1`. Le jar de Simple Voice Chat
//!   n'est de toute façon pas chargé côté serveur (`server::filter_server_mods`, le tunnel ne transporte pas l'UDP) ;
//! * **après `Done (`** : [`audit`] liste les sockets en écoute du processus Java (`/proc` sous Linux, `GetExtendedTcpTable`
//!   et `GetExtendedUdpTable` sous Windows). Toute écoute hors boucle locale arrête le serveur ([`exposure_message`]).
//!
//! Les fichiers de configuration viennent de mods tiers : ils sont lus bornés, réécrits ligne à ligne sans analyseur
//! complet, et un fichier absent n'est jamais créé (un fichier partiel pourrait casser la migration du mod).

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::types::Result;

// ---------------------------------------------------------------------------------------------------------------
// Chat vocal
// ---------------------------------------------------------------------------------------------------------------

/// Identifiants de Simple Voice Chat (`voicechat` dans `fabric.mod.json` / `mods.toml`).
pub const VOICE_CHAT_IDS: &[&str] = &["voicechat", "simple-voice-chat", "simple_voice_chat"];

/// Texte affiché quand le chat vocal est retiré du serveur.
pub const VOICE_UNSUPPORTED: &str = "Chat vocal non pris en charge";

pub fn is_voice_chat_id(id: &str) -> bool {
    VOICE_CHAT_IDS.iter().any(|v| v.eq_ignore_ascii_case(id))
}

/// Le nom du jar évoque Simple Voice Chat (ou un de ses addons) : repère rapide, sans ouvrir le jar, pour la carte.
pub fn is_voice_chat_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".jar") && ["voicechat", "voice-chat", "voice_chat"].iter().any(|k| n.contains(k))
}

// ---------------------------------------------------------------------------------------------------------------
// Réécriture des configurations (fonctions pures)
// ---------------------------------------------------------------------------------------------------------------

const LOOPBACK: &str = "127.0.0.1";

fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn is_comment(trimmed: &str) -> bool {
    trimmed.starts_with(['#', '!']) || trimmed.starts_with("//")
}

/// Joint des lignes avec la fin de ligne du fichier d'origine, plus une fin de ligne finale.
fn join_lines(lines: &[String], eol: &str) -> String {
    let mut out = lines.join(eol);
    out.push_str(eol);
    out
}

/// Fichier `.properties` : remplace la valeur de chaque clé de `forced` (la première occurrence, les doublons sont
/// supprimés) et ajoute à la fin celles qui manquent. Les commentaires et les autres clés sont gardés.
pub fn rewrite_properties(existing: &str, forced: &[(&str, String)]) -> String {
    let mut done = vec![false; forced.len()];
    let mut out = String::new();
    for line in existing.lines() {
        let key = line.trim_start().split(['=', ':']).next().unwrap_or("").trim();
        let idx = forced.iter().position(|(k, _)| !is_comment(line.trim_start()) && *k == key);
        match idx {
            Some(i) if !done[i] => {
                done[i] = true;
                out.push_str(&format!("{}={}\n", forced[i].0, forced[i].1));
            }
            // Doublon d'une clé déjà réécrite : supprimé.
            Some(_) => {}
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (i, (k, v)) in forced.iter().enumerate() {
        if !done[i] {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    out
}

/// YAML : force la valeur de la clé `path` (clés imbriquées repérées par l'indentation, sans analyseur complet).
/// `create_top_level` : une clé de premier niveau absente est ajoutée à la fin. Une clé imbriquée absente n'est pas
/// créée (le fichier est rendu tel quel). Les commentaires et le reste du fichier sont gardés.
pub fn render_yaml_key(existing: &str, path: &[&str], value: &str, create_top_level: bool) -> String {
    let eol = eol_of(existing);
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    // Pile des clés ouvertes : (indentation, clé).
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut found = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("- ") || trimmed == "-" {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let Some((raw_key, rest)) = trimmed.split_once(':') else { continue };
        // `clé:` suivi d'une valeur ou de rien ; « http://x » n'est pas une clé (pas d'espace après les deux-points).
        if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
            continue;
        }
        let key = raw_key.trim().trim_matches(['"', '\'']).to_string();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let is_target = stack.len() + 1 == path.len()
            && stack.iter().zip(path).all(|((_, k), p)| k == p)
            && key == path[path.len() - 1];
        if is_target {
            *line = format!("{}{}: {}", " ".repeat(indent), raw_key.trim(), value);
            found = true;
        } else if rest.trim().is_empty() || rest.trim().starts_with('#') {
            stack.push((indent, key));
        }
    }
    if !found && create_top_level && path.len() == 1 {
        lines.push(format!("{}: {}", path[0], value));
    }
    join_lines(&lines, eol)
}

/// HOCON (BlueMap) : force `key: "value"` pour chaque ligne active de cette clé à la racine, sinon l'ajoute à la fin
/// (en HOCON la dernière valeur l'emporte). Un document entouré d'accolades n'est pas touché.
pub fn render_hocon_key(existing: &str, key: &str, value: &str) -> String {
    if existing.trim_start().starts_with('{') {
        return existing.to_string();
    }
    let eol = eol_of(existing);
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut found = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if is_comment(trimmed) {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(key) else { continue };
        let rest = rest.trim_start();
        if rest.starts_with([':', '=']) {
            let indent = line.len() - trimmed.len();
            *line = format!("{}{key}: \"{value}\"", &line[..indent]);
            found = true;
        }
    }
    if !found {
        lines.push(format!("{key}: \"{value}\""));
    }
    join_lines(&lines, eol)
}

/// Façon de réécrire un fichier.
enum Edit {
    Yaml { path: &'static [&'static str], create_top_level: bool },
    Hocon { key: &'static str },
    Properties(&'static [(&'static str, &'static str)]),
}

/// Fichiers de config connus (relatifs au dossier du serveur) et ce qu'on y force.
const HARDENED: &[(&str, Edit)] = &[
    // Dynmap : `webserver-bindaddress: 0.0.0.0` par défaut.
    ("config/dynmap/configuration.txt", Edit::Yaml { path: &["webserver-bindaddress"], create_top_level: true }),
    // BlueMap : `ip` du serveur web intégré.
    ("config/bluemap/webserver.conf", Edit::Hocon { key: "ip" }),
    // Squaremap : `settings.internal-webserver.bind`.
    ("config/squaremap/config.yml", Edit::Yaml { path: &["settings", "internal-webserver", "bind"], create_top_level: false }),
    // Simple Voice Chat (au cas où son jar serait chargé quand même).
    ("config/voicechat/voicechat-server.properties", Edit::Properties(&[("bind_address", LOOPBACK), ("voice_host", "")])),
];

/// Nouveau contenu de `rel` (chemin relatif de [`HARDENED`]) d'après `existing`, `None` si `rel` n'est pas connu.
pub fn render_hardened(rel: &str, existing: &str) -> Option<String> {
    let (_, edit) = HARDENED.iter().find(|(p, _)| *p == rel)?;
    Some(match edit {
        Edit::Yaml { path, create_top_level } => render_yaml_key(existing, path, LOOPBACK, *create_top_level),
        Edit::Hocon { key } => render_hocon_key(existing, key, LOOPBACK),
        Edit::Properties(kv) => {
            let forced: Vec<(&str, String)> = kv.iter().map(|(k, v)| (*k, (*v).to_string())).collect();
            rewrite_properties(existing, &forced)
        }
    })
}

/// Taille maximale d'un fichier de config réécrit.
const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// Réécrit, dans le dossier du serveur `dir`, les configs connues qui existent et ne sont pas déjà liées à la boucle
/// locale. Rend les chemins relatifs réécrits. Un fichier absent, trop gros ou illisible est laissé tel quel : le
/// contrôle après `Done (` reste le filet.
pub async fn harden_configs(dir: &Path) -> Result<Vec<String>> {
    let mut rewritten = Vec::new();
    for (rel, _) in HARDENED {
        let path = dir.join(rel);
        let Ok(meta) = tokio::fs::metadata(&path).await else { continue };
        if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
            continue;
        }
        let Ok(existing) = tokio::fs::read_to_string(&path).await else { continue };
        let Some(new) = render_hardened(rel, &existing) else { continue };
        if new != existing {
            super::server::write_atomic(&path, new.as_bytes()).await?;
            rewritten.push((*rel).to_string());
        }
    }
    Ok(rewritten)
}

// ---------------------------------------------------------------------------------------------------------------
// Sockets en écoute
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

/// Un socket en écoute (TCP) ou lié (UDP) d'un processus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Listener {
    pub proto: Proto,
    pub addr: IpAddr,
    pub port: u16,
}

/// Adresse de la boucle locale : `127.0.0.0/8`, `::1`, et `::ffff:127.x.x.x`.
pub fn is_loopback(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(a) => a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Les écoutes joignables depuis le réseau (tout ce qui n'est pas la boucle locale : `0.0.0.0`, `::`, IP du réseau local).
pub fn exposed(all: &[Listener]) -> Vec<Listener> {
    let mut v: Vec<Listener> = all.iter().filter(|l| !is_loopback(&l.addr)).copied().collect();
    v.sort_by_key(|l| (l.proto == Proto::Udp, l.port));
    v.dedup_by_key(|l| (l.proto, l.port));
    v
}

/// Un socket lu dans `/proc/net/*`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcSocket {
    pub addr: IpAddr,
    pub port: u16,
    pub inode: u64,
}

/// Lit `/proc/net/{tcp,tcp6,udp,udp6}` : les sockets en écoute (TCP état `0A`) ou liés non connectés (UDP état `07`).
/// Une ligne mal formée est ignorée.
pub fn parse_proc_net(text: &str, proto: Proto) -> Vec<ProcSocket> {
    let want = match proto {
        Proto::Tcp => "0A",
        Proto::Udp => "07",
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !f[3].eq_ignore_ascii_case(want) {
            continue;
        }
        let Some((a, p)) = f[1].split_once(':') else { continue };
        let (Some(addr), Ok(port), Ok(inode)) = (parse_proc_addr(a), u16::from_str_radix(p, 16), f[9].parse::<u64>()) else {
            continue;
        };
        if inode != 0 {
            out.push(ProcSocket { addr, port, inode });
        }
    }
    out
}

/// Adresse de `/proc/net/*` : chaque mot de 32 bits est écrit dans l'ordre des octets de la machine (petit-boutiste).
fn parse_proc_addr(hex: &str) -> Option<IpAddr> {
    let word = |i: usize| u32::from_str_radix(hex.get(i * 8..i * 8 + 8)?, 16).ok().map(u32::to_le_bytes);
    match hex.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(0)?))),
        32 => {
            let mut b = [0u8; 16];
            for i in 0..4 {
                b[i * 4..i * 4 + 4].copy_from_slice(&word(i)?);
            }
            Some(IpAddr::V6(Ipv6Addr::from(b)))
        }
        _ => None,
    }
}

/// Lit une table de `GetExtendedTcpTable` (`TCP_TABLE_OWNER_PID_LISTENER`) ou de `GetExtendedUdpTable`
/// (`UDP_TABLE_OWNER_PID`) et rend les sockets du processus `pid`. Le tampon est un compte de lignes (`u32`) suivi des
/// lignes ; tout est en `u32` petit-boutiste, le port est dans les 16 bits bas, en ordre réseau.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_ip_table(buf: &[u8], proto: Proto, v6: bool, pid: u32) -> Vec<Listener> {
    // (taille de la ligne, décalage de l'adresse, du port, du PID) ; la ligne TCP v4 commence par l'état.
    let (row, addr_off, port_off, pid_off) = match (proto, v6) {
        (Proto::Tcp, false) => (24, 4, 8, 20),
        (Proto::Tcp, true) => (56, 0, 20, 52),
        (Proto::Udp, false) => (12, 0, 4, 8),
        (Proto::Udp, true) => (28, 0, 20, 24),
    };
    let u32_at = |b: &[u8], o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let Some(n) = u32_at(buf, 0) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..n as usize {
        let Some(r) = buf.get(4 + i * row..4 + (i + 1) * row) else { break };
        let (Some(port), Some(owner)) = (u32_at(r, port_off), u32_at(r, pid_off)) else { break };
        if owner != pid {
            continue;
        }
        let addr = if v6 {
            let mut a = [0u8; 16];
            a.copy_from_slice(&r[addr_off..addr_off + 16]);
            IpAddr::V6(Ipv6Addr::from(a))
        } else {
            IpAddr::V4(Ipv4Addr::new(r[addr_off], r[addr_off + 1], r[addr_off + 2], r[addr_off + 3]))
        };
        out.push(Listener { proto, addr, port: u16::from_be((port & 0xFFFF) as u16) });
    }
    out
}

/// Les sockets en écoute ou liés du processus `pid` (seulement lui, pas ses enfants : le serveur est le Java lancé
/// directement).
#[cfg(target_os = "linux")]
pub fn listeners_of(pid: u32) -> std::io::Result<Vec<Listener>> {
    let mut inodes: HashSet<u64> = HashSet::new();
    for e in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let Ok(target) = std::fs::read_link(e?.path()) else { continue };
        if let Some(n) = target.to_string_lossy().strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
            if let Ok(n) = n.parse() {
                inodes.insert(n);
            }
        }
    }
    let mut out = Vec::new();
    for (file, proto) in [("tcp", Proto::Tcp), ("tcp6", Proto::Tcp), ("udp", Proto::Udp), ("udp6", Proto::Udp)] {
        // La table du processus (son espace de noms réseau), pas celle de l'appelant.
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{file}")) else { continue };
        for s in parse_proc_net(&text, proto) {
            if inodes.contains(&s.inode) {
                out.push(Listener { proto, addr: s.addr, port: s.port });
            }
        }
    }
    Ok(out)
}

#[cfg(windows)]
pub fn listeners_of(pid: u32) -> std::io::Result<Vec<Listener>> {
    let mut out = Vec::new();
    for (proto, v6) in [(Proto::Tcp, false), (Proto::Tcp, true), (Proto::Udp, false), (Proto::Udp, true)] {
        let buf = win_table(proto, v6)?;
        out.extend(parse_ip_table(&buf, proto, v6, pid));
    }
    Ok(out)
}

/// Table des sockets de tout le système, par `GetExtendedTcpTable` / `GetExtendedUdpTable`.
#[cfg(windows)]
fn win_table(proto: Proto, v6: bool) -> std::io::Result<Vec<u8>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, TCP_TABLE_OWNER_PID_LISTENER, UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let family = u32::from(if v6 { AF_INET6 } else { AF_INET });
    let mut buf: Vec<u8> = vec![0; 4096];
    // La table peut grandir entre deux appels : quelques essais.
    for _ in 0..8 {
        let mut size = buf.len() as u32;
        let ptr = buf.as_mut_ptr().cast::<core::ffi::c_void>();
        // SAFETY : `ptr` désigne `size` octets écrivables, `buf` vit pendant l'appel ; l'API n'écrit pas au-delà de `size`.
        let rc = unsafe {
            match proto {
                Proto::Tcp => GetExtendedTcpTable(ptr, &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0),
                Proto::Udp => GetExtendedUdpTable(ptr, &mut size, 0, family, UDP_TABLE_OWNER_PID, 0),
            }
        };
        match rc {
            0 => {
                buf.truncate((size as usize).min(buf.len()));
                return Ok(buf);
            }
            ERROR_INSUFFICIENT_BUFFER => buf.resize(size as usize + 4096, 0),
            e => return Err(std::io::Error::from_raw_os_error(e as i32)),
        }
    }
    Err(std::io::Error::other("table des sockets instable"))
}

/// Plateformes sans lecture des sockets : aucun contrôle possible.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn listeners_of(_pid: u32) -> std::io::Result<Vec<Listener>> {
    Ok(Vec::new())
}

/// Délai entre les deux lectures qui confirment une écoute UDP.
const UDP_CONFIRM: Duration = Duration::from_millis(600);

/// Les écoutes non loopback du processus `pid`. Une écoute UDP n'est retenue que si elle est encore là à une
/// seconde lecture (les sockets UDP éphémères de Java ne sont pas des écoutes). Si la lecture est impossible, rien
/// n'est signalé (un contrôle impossible n'est pas une exposition) et la cause est journalisée.
pub async fn audit(pid: u32) -> Vec<Listener> {
    let read = move || tokio::task::spawn_blocking(move || listeners_of(pid));
    let first = match read().await {
        Ok(Ok(v)) => exposed(&v),
        Ok(Err(e)) => {
            tracing::warn!("contrôle des sockets du serveur impossible : {e}");
            return Vec::new();
        }
        Err(_) => return Vec::new(),
    };
    if !first.iter().any(|l| l.proto == Proto::Udp) {
        return first;
    }
    tokio::time::sleep(UDP_CONFIRM).await;
    let second: HashSet<(Proto, u16)> = match read().await {
        Ok(Ok(v)) => exposed(&v).iter().map(|l| (l.proto, l.port)).collect(),
        _ => HashSet::new(),
    };
    first.into_iter().filter(|l| l.proto == Proto::Tcp || second.contains(&(l.proto, l.port))).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Mod probable, message
// ---------------------------------------------------------------------------------------------------------------

/// Mods connus pour ouvrir un port : (mot cherché en minuscules, nom affiché, port par défaut).
const KNOWN_PORTS: &[(&str, &str, u16)] = &[
    ("bluemap", "BlueMap", 8100),
    ("dynmap", "Dynmap", 8123),
    ("squaremap", "Squaremap", 8080),
    ("voicechat", "Simple Voice Chat", 24454),
];

fn friendly(name: &str) -> String {
    let low = name.to_ascii_lowercase();
    KNOWN_PORTS.iter().find(|(k, _, _)| low.contains(k)).map_or_else(|| name.to_string(), |(_, n, _)| (*n).to_string())
}

/// `line` contient le nombre `port` comme mot entier (pas `18100`, pas `81000`).
fn has_port_token(line: &str, port: u16) -> bool {
    let p = port.to_string();
    line.match_indices(&p).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = line[i + p.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Le mod probable derrière une écoute sur `port` : le dossier ou le fichier de `config/` qui déclare ce port (ligne
/// contenant « port »), puis les lignes du journal qui le citent avec le nom d'un mod connu, puis le port par défaut
/// d'un mod connu. `None` : inconnu. Heuristique d'affichage seulement.
pub fn probable_mod(port: u16, dir: &Path, log: &[String]) -> Option<String> {
    if let Some(m) = config_mentioning(&dir.join("config"), port, 3) {
        return Some(friendly(&m));
    }
    for line in log {
        let low = line.to_ascii_lowercase();
        if has_port_token(&low, port) {
            if let Some((_, name, _)) = KNOWN_PORTS.iter().find(|(k, _, _)| low.contains(k)) {
                return Some((*name).to_string());
            }
        }
    }
    KNOWN_PORTS.iter().find(|(_, _, p)| *p == port).map(|(_, n, _)| (*n).to_string())
}

/// Premier élément de `root` (dossier ou fichier, ordre alphabétique) dont un fichier de config déclare `port`.
fn config_mentioning(root: &Path, port: u16, depth: u32) -> Option<String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for top in entries {
        let name = top.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = if top.is_dir() { top.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name) } else { name };
        if file_declares_port(&top, port, depth) {
            return Some(name);
        }
    }
    None
}

fn file_declares_port(path: &Path, port: u16, depth: u32) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if meta.is_dir() {
        if depth == 0 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(path) else { return false };
        let mut children: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        return children.iter().any(|c| file_declares_port(c, port, depth - 1));
    }
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return false;
    }
    let text_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["toml", "conf", "yml", "yaml", "txt", "properties", "json", "json5", "cfg"].iter().any(|x| x.eq_ignore_ascii_case(e)));
    if !text_ext {
        return false;
    }
    std::fs::read_to_string(path)
        .map(|t| t.lines().any(|l| l.to_ascii_lowercase().contains("port") && has_port_token(l, port)))
        .unwrap_or(false)
}

/// Message de l'arrêt du serveur : « BlueMap ouvre le port 8100 sur ton réseau : le serveur a été arrêté. »
pub fn exposure_message(l: &Listener, mod_name: Option<&str>) -> String {
    let who = mod_name.map_or_else(|| "Un mod du serveur".to_string(), str::to_string);
    let kind = if l.proto == Proto::Udp { "UDP " } else { "" };
    format!(
        "{who} ouvre le port {kind}{} sur ton réseau : le serveur a été arrêté. Les amis passent par le tunnel de Cubelink, aucun port ne doit être ouvert. Retire ce mod du modpack, ou désactive son serveur web, puis réessaie.",
        l.port
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DYNMAP: &str = "# Dynmap\r\ndeftemplatesuffix: v1\r\nwebserver-bindaddress: 0.0.0.0\r\nwebserver-port: 8123\r\nstorage:\r\n  type: filetree\r\n";
    const BLUEMAP: &str = "## BlueMap\nenabled: true\nwebroot: \"bluemap/web\"\n\n# The IP-Address that the webserver binds to\n# Use 0.0.0.0 to bind to all\nip: \"0.0.0.0\"\n\nport: 8100\n";
    const SQUAREMAP: &str = "config-version: 1\nsettings:\n  internal-webserver:\n    enabled: true\n    bind: 0.0.0.0 # all\n    port: 8080\n  other:\n    bind: 1.2.3.4\n";

    #[test]
    fn dynmap_key_is_forced_and_crlf_kept() {
        let out = render_hardened("config/dynmap/configuration.txt", DYNMAP).unwrap();
        assert!(out.contains("webserver-bindaddress: 127.0.0.1\r\n"), "{out:?}");
        assert!(out.contains("webserver-port: 8123\r\n") && out.contains("  type: filetree\r\n"));
        assert!(!out.contains("0.0.0.0"));
        // Idempotent.
        assert_eq!(render_hardened("config/dynmap/configuration.txt", &out).unwrap(), out);
    }

    #[test]
    fn dynmap_missing_key_is_appended() {
        let out = render_hardened("config/dynmap/configuration.txt", "webserver-port: 8123").unwrap();
        assert_eq!(out, "webserver-port: 8123\nwebserver-bindaddress: 127.0.0.1\n");
    }

    #[test]
    fn bluemap_ip_is_forced_and_comments_kept() {
        let out = render_hardened("config/bluemap/webserver.conf", BLUEMAP).unwrap();
        assert!(out.contains("\nip: \"127.0.0.1\"\n"), "{out}");
        assert!(out.contains("# Use 0.0.0.0 to bind to all"), "le commentaire reste");
        assert_eq!(out.matches("127.0.0.1").count(), 1);
        assert_eq!(render_hardened("config/bluemap/webserver.conf", &out).unwrap(), out);
    }

    #[test]
    fn bluemap_missing_or_equals_key() {
        let out = render_hardened("config/bluemap/webserver.conf", "enabled: true\n#ip: \"0.0.0.0\"\n").unwrap();
        assert!(out.ends_with("ip: \"127.0.0.1\"\n"), "{out}");
        let out = render_hardened("config/bluemap/webserver.conf", "  ip = \"0.0.0.0\"\nport = 1\n").unwrap();
        assert_eq!(out, "  ip: \"127.0.0.1\"\nport = 1\n");
        // Document entre accolades : laissé tel quel (le contrôle après `Done (` est le filet).
        let braces = "{ ip: \"0.0.0.0\" }\n";
        assert_eq!(render_hardened("config/bluemap/webserver.conf", braces).unwrap(), braces);
        // Une clé qui commence pareil n'est pas `ip`.
        let out = render_hardened("config/bluemap/webserver.conf", "ipv6: true\n").unwrap();
        assert_eq!(out, "ipv6: true\nip: \"127.0.0.1\"\n");
    }

    #[test]
    fn squaremap_nested_bind_only() {
        let out = render_hardened("config/squaremap/config.yml", SQUAREMAP).unwrap();
        assert!(out.contains("    bind: 127.0.0.1\n"), "{out}");
        assert!(out.contains("    bind: 1.2.3.4\n"), "le `bind` d'une autre section reste");
        assert!(out.contains("    port: 8080\n"));
        assert_eq!(render_hardened("config/squaremap/config.yml", &out).unwrap(), out);
        // Section absente : rien n'est créé.
        assert_eq!(render_hardened("config/squaremap/config.yml", "settings:\n  x: 1\n").unwrap(), "settings:\n  x: 1\n");
    }

    #[test]
    fn voicechat_properties_are_forced() {
        let src = "# SVC\nport=24454\nbind_address=\nvoice_host=voice.example.org\nbind_address=0.0.0.0\n";
        let out = render_hardened("config/voicechat/voicechat-server.properties", src).unwrap();
        assert_eq!(out, "# SVC\nport=24454\nbind_address=127.0.0.1\nvoice_host=\n");
    }

    #[test]
    fn unknown_file_is_none() {
        assert!(render_hardened("config/autre.toml", "a=1").is_none());
    }

    #[tokio::test]
    async fn harden_rewrites_existing_files_only() {
        let t = tempfile::tempdir().unwrap();
        let cfg = t.path().join("config/bluemap");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("webserver.conf"), BLUEMAP).unwrap();
        let done = harden_configs(t.path()).await.unwrap();
        assert_eq!(done, vec!["config/bluemap/webserver.conf".to_string()]);
        assert!(std::fs::read_to_string(cfg.join("webserver.conf")).unwrap().contains("ip: \"127.0.0.1\""));
        // Les mods absents n'ont pas de fichier créé.
        assert!(!t.path().join("config/dynmap").exists());
        assert!(!t.path().join("config/squaremap").exists());
        assert!(!t.path().join("config/voicechat").exists());
        // Deuxième passe : plus rien à faire.
        assert!(harden_configs(t.path()).await.unwrap().is_empty());
    }

    #[test]
    fn voice_chat_detection() {
        assert!(is_voice_chat_id("voicechat") && is_voice_chat_id("Simple-Voice-Chat") && !is_voice_chat_id("lithium"));
        assert!(is_voice_chat_file("voicechat-fabric-1.21.1-2.5.26.jar") && is_voice_chat_file("Simple-Voice-Chat.JAR"));
        assert!(!is_voice_chat_file("lithium.jar") && !is_voice_chat_file("voicechat.txt"));
    }

    // Échantillons réels de /proc/net (Linux 6.x).
    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 100 0 0 10 0\n   1: 00000000:1FA4 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0000000000000000 100 0 0 10 0\n   2: 0100007F:1F90 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0000000000000000 100 0 0 10 0\n";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 44444 1 0000000000000000 100 0 0 10 0\n   1: 00000000000000000000000000000000:1FA4 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 55555 1 0000000000000000 100 0 0 10 0\n   2: 0000000000000000FFFF00000100007F:1FA5 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 66666 1 0000000000000000 100 0 0 10 0\n";
    const UDP: &str = "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n  100: 00000000:5EC6 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 77777 2 0000000000000000 0\n  101: 0100007F:0035 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 88888 2 0000000000000000 0\n";

    #[test]
    fn proc_net_parsing() {
        let tcp = parse_proc_net(TCP, Proto::Tcp);
        assert_eq!(tcp.len(), 2, "la connexion établie (état 01) n'est pas une écoute");
        assert_eq!((tcp[0].addr, tcp[0].port, tcp[0].inode), (IpAddr::V4(Ipv4Addr::LOCALHOST), 8080, 11111));
        assert_eq!((tcp[1].addr, tcp[1].port), (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8100));
        let tcp6 = parse_proc_net(TCP6, Proto::Tcp);
        assert_eq!(tcp6.len(), 3);
        assert_eq!(tcp6[0].addr, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(tcp6[1].addr, IpAddr::V6(Ipv6Addr::UNSPECIFIED));
        assert_eq!(tcp6[2].addr, IpAddr::V6("::ffff:127.0.0.1".parse().unwrap()));
        let udp = parse_proc_net(UDP, Proto::Udp);
        assert_eq!(udp.len(), 1);
        assert_eq!((udp[0].addr, udp[0].port), (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 24262));
        assert!(parse_proc_net("", Proto::Tcp).is_empty());
        assert!(parse_proc_net("h\nn'importe quoi\n", Proto::Tcp).is_empty());
    }

    #[test]
    fn loopback_and_exposed() {
        let l = |a: &str, port, proto| Listener { proto, addr: a.parse().unwrap(), port };
        for ok in ["127.0.0.1", "127.5.5.5", "::1", "::ffff:127.0.0.1"] {
            assert!(is_loopback(&ok.parse().unwrap()), "{ok}");
        }
        for bad in ["0.0.0.0", "::", "192.168.1.20", "10.0.0.2", "fe80::1", "::ffff:192.168.1.5"] {
            assert!(!is_loopback(&bad.parse().unwrap()), "{bad}");
        }
        let all = [
            l("127.0.0.1", 25565, Proto::Tcp),
            l("0.0.0.0", 8100, Proto::Tcp),
            l("::", 8100, Proto::Tcp),
            l("192.168.1.20", 24454, Proto::Udp),
            l("::1", 9000, Proto::Tcp),
        ];
        let e = exposed(&all);
        assert_eq!(e.iter().map(|x| (x.proto, x.port)).collect::<Vec<_>>(), vec![(Proto::Tcp, 8100), (Proto::Udp, 24454)]);
    }

    #[test]
    fn windows_tables_parse() {
        let pid = 4242u32;
        let port_be = |p: u16| u32::from(p.to_be());
        let u = |v: u32| v.to_le_bytes();
        // TCP v4 : 2 lignes, une du processus (0.0.0.0:8100), une d'un autre.
        let mut buf = u(2).to_vec();
        for (addr, port, owner) in [([0u8, 0, 0, 0], 8100, pid), ([127, 0, 0, 1], 25565, 1)] {
            buf.extend(u(2)); // état
            buf.extend(addr);
            buf.extend(u(port_be(port)));
            buf.extend(u(0));
            buf.extend(u(0));
            buf.extend(u(owner));
        }
        let got = parse_ip_table(&buf, Proto::Tcp, false, pid);
        assert_eq!(got, vec![Listener { proto: Proto::Tcp, addr: "0.0.0.0".parse().unwrap(), port: 8100 }]);
        // UDP v6 : ::1 puis ::.
        let mut buf = u(2).to_vec();
        let mut lo = [0u8; 16];
        lo[15] = 1;
        for (addr, port) in [(lo, 24454u16), ([0u8; 16], 24455)] {
            buf.extend(addr);
            buf.extend(u(0)); // scope
            buf.extend(u(port_be(port)));
            buf.extend(u(pid));
        }
        let got = parse_ip_table(&buf, Proto::Udp, true, pid);
        assert_eq!(got.len(), 2);
        assert_eq!(exposed(&got), vec![Listener { proto: Proto::Udp, addr: "::".parse().unwrap(), port: 24455 }]);
        // TCP v6.
        let mut buf = u(1).to_vec();
        buf.extend([0u8; 16]);
        buf.extend(u(0));
        buf.extend(u(port_be(8123)));
        buf.extend([0u8; 16]);
        buf.extend(u(0));
        buf.extend(u(0));
        buf.extend(u(2));
        buf.extend(u(pid));
        assert_eq!(parse_ip_table(&buf, Proto::Tcp, true, pid)[0].port, 8123);
        // Tampon tronqué ou vide : pas de panique.
        assert!(parse_ip_table(&buf[..10], Proto::Tcp, true, pid).is_empty());
        assert!(parse_ip_table(&[], Proto::Tcp, false, pid).is_empty());
    }

    #[test]
    fn probable_mod_from_config_log_and_defaults() {
        let t = tempfile::tempdir().unwrap();
        let cfg = t.path().join("config");
        std::fs::create_dir_all(cfg.join("bluemap")).unwrap();
        std::fs::write(cfg.join("bluemap/webserver.conf"), "ip: \"0.0.0.0\"\nport: 9191\n").unwrap();
        std::fs::write(cfg.join("autre.toml"), "max_players = 9191\n").unwrap();
        assert_eq!(probable_mod(9191, t.path(), &[]).as_deref(), Some("BlueMap"), "dossier du mod qui déclare le port");
        // 19191 n'est pas 9191.
        assert_eq!(probable_mod(191, t.path(), &[]), None);
        // Le journal nomme le mod.
        let log = vec!["[12:00:00] [dynmap/INFO]: Web server started on port 7777".to_string()];
        assert_eq!(probable_mod(7777, t.path(), &log).as_deref(), Some("Dynmap"));
        // Port par défaut connu.
        assert_eq!(probable_mod(8123, t.path(), &[]).as_deref(), Some("Dynmap"));
        assert_eq!(probable_mod(31337, t.path(), &[]), None);
    }

    #[test]
    fn message_names_mod_and_port() {
        let l = Listener { proto: Proto::Tcp, addr: "0.0.0.0".parse().unwrap(), port: 8100 };
        assert!(exposure_message(&l, Some("BlueMap")).starts_with("BlueMap ouvre le port 8100 sur ton réseau : le serveur a été arrêté."));
        assert!(exposure_message(&l, None).starts_with("Un mod du serveur ouvre le port 8100"));
        let u = Listener { proto: Proto::Udp, ..l };
        assert!(exposure_message(&u, None).contains("port UDP 8100"));
    }
}
