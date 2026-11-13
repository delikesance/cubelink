//! Plages de versions des dépendances de mods : syntaxe Fabric / Quilt (`>=1.0 <2`, `~1.20.1`, `1.20.x`, `a || b`)
//! et syntaxe Maven de Forge / NeoForge (`[1.20.1,1.21)`, `[1.0,)`, `[1.20.1]`, version nue = « au moins »).
//!
//! `matches` renvoie `None` quand c'est indécidable (version ou plage illisible) : l'appelant ne signale alors rien.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Fabric,
    Maven,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    N(u64),
    S(String),
}

impl Ord for Tok {
    fn cmp(&self, o: &Self) -> Ordering {
        match (self, o) {
            (Tok::N(a), Tok::N(b)) => a.cmp(b),
            (Tok::N(_), Tok::S(_)) => Ordering::Less,
            (Tok::S(_), Tok::N(_)) => Ordering::Greater,
            (Tok::S(a), Tok::S(b)) => a.cmp(b),
        }
    }
}
impl PartialOrd for Tok {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ver {
    /// `1.21.1`, `21.1.250`, `1.0-beta.2` : `pre` = `None` pour une version finale (la plus grande).
    Release { nums: Vec<u64>, pre: Option<Vec<Tok>> },
    /// `24w14a`.
    Snapshot(u64, u64, char),
}

fn pre_tokens(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for part in s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()) {
        let mut cur = String::new();
        let mut digit = false;
        for c in part.chars() {
            let d = c.is_ascii_digit();
            if !cur.is_empty() && d != digit {
                out.push(mk_tok(&cur, digit));
                cur.clear();
            }
            digit = d;
            cur.push(c.to_ascii_lowercase());
        }
        if !cur.is_empty() {
            out.push(mk_tok(&cur, digit));
        }
    }
    out
}

fn mk_tok(s: &str, digit: bool) -> Tok {
    if digit {
        s.parse().map(Tok::N).unwrap_or_else(|_| Tok::S(s.to_string()))
    } else {
        Tok::S(s.to_string())
    }
}

fn parse_snapshot(s: &str) -> Option<Ver> {
    let b = s.as_bytes();
    if b.len() == 6 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b'w' && b[3].is_ascii_digit() && b[4].is_ascii_digit() && b[5].is_ascii_lowercase() {
        let y = s[0..2].parse().ok()?;
        let w = s[3..5].parse().ok()?;
        return Some(Ver::Snapshot(y, w, b[5] as char));
    }
    None
}

fn parse_version(s: &str) -> Option<Ver> {
    let s = s.trim();
    let s = s.split('+').next().unwrap_or("");
    if s.is_empty() || s.contains("${") {
        return None;
    }
    if let Some(v) = parse_snapshot(s) {
        return Some(v);
    }
    let (main, pre) = match s.split_once('-') {
        Some((m, p)) => (m, Some(pre_tokens(p))),
        None => (s, None),
    };
    let nums: Option<Vec<u64>> = main.split('.').map(|p| p.parse().ok()).collect();
    let nums = nums?;
    if nums.is_empty() {
        return None;
    }
    Some(Ver::Release { nums, pre })
}

fn cmp_ver(a: &Ver, b: &Ver) -> Option<Ordering> {
    match (a, b) {
        (Ver::Snapshot(a1, a2, a3), Ver::Snapshot(b1, b2, b3)) => Some((a1, a2, a3).cmp(&(b1, b2, b3))),
        (Ver::Release { nums: an, pre: ap }, Ver::Release { nums: bn, pre: bp }) => {
            // `26.1.2-12.5.5` (version de Minecraft en préfixe) contre `12.6.12` : formats incomparables.
            let numeric_pre = |p: &Option<Vec<Tok>>| matches!(p, Some(t) if matches!(t.first(), Some(Tok::N(_))));
            if numeric_pre(ap) != numeric_pre(bp) {
                return None;
            }
            let n = an.len().max(bn.len());
            for i in 0..n {
                let o = an.get(i).copied().unwrap_or(0).cmp(&bn.get(i).copied().unwrap_or(0));
                if o != Ordering::Equal {
                    return Some(o);
                }
            }
            Some(match (ap, bp) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(x), Some(y)) => x.cmp(y),
            })
        }
        _ => None,
    }
}

/// Compare deux versions ; `None` si l'une est illisible ou si on mélange snapshot et version finale.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    cmp_ver(&parse_version(a)?, &parse_version(b)?)
}

/// Plage vide ou `*` : toute version convient.
pub fn is_any(range: &str) -> bool {
    let r = range.trim();
    r.is_empty() || r == "*"
}

/// `Some(true)` si `version` est dans `range`, `Some(false)` si elle n'y est pas, `None` si on ne peut pas savoir.
pub fn matches(range: &str, version: &str, flavor: Flavor) -> Option<bool> {
    if is_any(range) {
        return Some(true);
    }
    let v = parse_version(version)?;
    match flavor {
        Flavor::Fabric => fabric_matches(range, &v),
        Flavor::Maven => maven_matches(range, &v),
    }
}

// ---------- Fabric ----------

fn fabric_matches(range: &str, v: &Ver) -> Option<bool> {
    let mut unknown = false;
    for alt in range.split("||") {
        match fabric_and(alt, v) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

fn fabric_and(alt: &str, v: &Ver) -> Option<bool> {
    let mut res = Some(true);
    for t in alt.split_whitespace() {
        match fabric_cmp(t, v) {
            Some(false) => return Some(false),
            None => res = None,
            Some(true) => {}
        }
    }
    res
}

fn is_wild(seg: &str) -> bool {
    matches!(seg, "x" | "X" | "*")
}

fn release_nums(v: &Ver) -> Option<&Vec<u64>> {
    match v {
        Ver::Release { nums, .. } => Some(nums),
        Ver::Snapshot(..) => None,
    }
}

fn fabric_cmp(tok: &str, v: &Ver) -> Option<bool> {
    if is_wild(tok) {
        return Some(true);
    }
    let (op, rest) = ["<=", ">=", "<", ">", "=", "~", "^"].iter().find_map(|p| tok.strip_prefix(p).map(|r| (*p, r))).unwrap_or(("", tok));
    let segs: Vec<&str> = rest.split('.').collect();
    if segs.iter().any(|s| is_wild(s)) {
        let keep: Vec<&str> = segs.iter().take_while(|s| !is_wild(s)).copied().collect();
        let nums: Option<Vec<u64>> = keep.iter().map(|s| s.parse().ok()).collect();
        let nums = nums?;
        return match op {
            "" | "=" => {
                let vn = release_nums(v)?;
                Some(nums.iter().enumerate().all(|(i, n)| vn.get(i).copied().unwrap_or(0) == *n))
            }
            ">=" | "<" => {
                let lo = Ver::Release { nums, pre: None };
                let o = cmp_ver(v, &lo)?;
                Some(if op == ">=" { o != Ordering::Less } else { o == Ordering::Less })
            }
            _ => None,
        };
    }
    let b = parse_version(rest)?;
    match op {
        "" | "=" => cmp_ver(v, &b).map(|o| o == Ordering::Equal),
        ">=" => cmp_ver(v, &b).map(|o| o != Ordering::Less),
        ">" => cmp_ver(v, &b).map(|o| o == Ordering::Greater),
        "<=" => cmp_ver(v, &b).map(|o| o != Ordering::Greater),
        "<" => cmp_ver(v, &b).map(|o| o == Ordering::Less),
        "~" | "^" => {
            let Ver::Release { nums, .. } = &b else { return None };
            let hi = if op == "^" || nums.len() < 2 { vec![nums[0] + 1] } else { vec![nums[0], nums[1] + 1] };
            let hi = Ver::Release { nums: hi, pre: Some(vec![]) };
            let lo_ok = cmp_ver(v, &b)? != Ordering::Less;
            let hi_ok = cmp_ver(v, &hi)? == Ordering::Less;
            Some(lo_ok && hi_ok)
        }
        _ => None,
    }
}

// ---------- Maven ----------

/// Borne : version + « incluse ».
type Bound = Option<(String, bool)>;

struct Interval {
    lo: Bound,
    hi: Bound,
    exact: Option<String>,
}

fn maven_intervals(range: &str) -> Option<Vec<Interval>> {
    let r = range.trim();
    if !r.starts_with('[') && !r.starts_with('(') {
        if r.contains(',') || r.is_empty() {
            return None;
        }
        return Some(vec![Interval { lo: Some((r.to_string(), true)), hi: None, exact: None }]);
    }
    let mut out = Vec::new();
    let mut rest = r;
    loop {
        rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        if rest.is_empty() {
            break;
        }
        let open = rest.chars().next()?;
        if open != '[' && open != '(' {
            return None;
        }
        let end = rest.find([']', ')'])?;
        let close = rest[end..].chars().next()?;
        let inner = &rest[1..end];
        let (lo_inc, hi_inc) = (open == '[', close == ']');
        match inner.split_once(',') {
            Some((lo, hi)) => {
                let (lo, hi) = (lo.trim(), hi.trim());
                out.push(Interval {
                    lo: (!lo.is_empty()).then(|| (lo.to_string(), lo_inc)),
                    hi: (!hi.is_empty()).then(|| (hi.to_string(), hi_inc)),
                    exact: None,
                });
            }
            None => {
                if !(lo_inc && hi_inc) || inner.trim().is_empty() {
                    return None;
                }
                out.push(Interval { lo: None, hi: None, exact: Some(inner.trim().to_string()) });
            }
        }
        rest = &rest[end + 1..];
    }
    (!out.is_empty()).then_some(out)
}

fn maven_matches(range: &str, v: &Ver) -> Option<bool> {
    let ivs = maven_intervals(range)?;
    let mut unknown = false;
    for iv in &ivs {
        match interval_has(iv, v) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

fn interval_has(iv: &Interval, v: &Ver) -> Option<bool> {
    if let Some(e) = &iv.exact {
        return cmp_ver(v, &parse_version(e)?).map(|o| o == Ordering::Equal);
    }
    if let Some((lo, inc)) = &iv.lo {
        let o = cmp_ver(v, &parse_version(lo)?)?;
        if o == Ordering::Less || (o == Ordering::Equal && !inc) {
            return Some(false);
        }
    }
    if let Some((hi, inc)) = &iv.hi {
        let o = cmp_ver(v, &parse_version(hi)?)?;
        if o == Ordering::Greater || (o == Ordering::Equal && !inc) {
            return Some(false);
        }
    }
    Some(true)
}

/// `true` si une borne de la plage Maven partage `major.minor` avec `version` (ex. `[1.21,1.21.1)` pour `1.21.1`).
pub fn same_family(range: &str, version: &str) -> bool {
    let Some(Ver::Release { nums: vn, .. }) = parse_version(version) else { return false };
    let fam = |s: &str| match parse_version(s) {
        Some(Ver::Release { nums, .. }) => nums.len() >= 2 && vn.len() >= 2 && nums[0] == vn[0] && nums[1] == vn[1],
        _ => false,
    };
    maven_intervals(range).is_some_and(|ivs| {
        ivs.iter().any(|iv| iv.exact.as_deref().is_some_and(fam) || iv.lo.as_ref().is_some_and(|(v, _)| fam(v)) || iv.hi.as_ref().is_some_and(|(v, _)| fam(v)))
    })
}

// ---------- Texte lisible ----------

fn fabric_intervals(range: &str) -> Option<Vec<Interval>> {
    let mut out = Vec::new();
    for alt in range.split("||") {
        let mut iv = Interval { lo: None, hi: None, exact: None };
        for t in alt.split_whitespace() {
            if is_wild(t) {
                continue;
            }
            let (op, rest) = ["<=", ">=", "<", ">", "="].iter().find_map(|p| t.strip_prefix(p).map(|r| (*p, r))).unwrap_or(("", t));
            if rest.is_empty() || rest.starts_with(['~', '^']) || rest.split('.').any(is_wild) {
                return None;
            }
            let s = rest.to_string();
            match op {
                ">=" => iv.lo = Some((s, true)),
                ">" => iv.lo = Some((s, false)),
                "<=" => iv.hi = Some((s, true)),
                "<" => iv.hi = Some((s, false)),
                _ => iv.exact = Some(s),
            }
        }
        out.push(iv);
    }
    (!out.is_empty()).then_some(out)
}

fn fmt_interval(iv: &Interval) -> String {
    if let Some(e) = &iv.exact {
        return e.clone();
    }
    match (&iv.lo, &iv.hi) {
        (None, None) => "toute version".into(),
        (Some((v, true)), None) => format!("{v} ou plus récent"),
        (Some((v, false)), None) => format!("plus récent que {v}"),
        (None, Some((v, true))) => format!("{v} ou plus ancien"),
        (None, Some((v, false))) => format!("plus ancien que {v}"),
        (Some((a, ai)), Some((b, bi))) => {
            let a = if *ai { a.clone() } else { format!("{a} (exclu)") };
            let b = if *bi { b.clone() } else { format!("{b} (exclu)") };
            format!("entre {a} et {b}")
        }
    }
}

/// Plage en français court (« 1.0 ou plus récent », « entre 1.20.1 et 1.21 (exclu) »). Syntaxe inconnue : texte tel quel.
pub fn describe(range: &str, flavor: Flavor) -> String {
    let ivs = match flavor {
        Flavor::Fabric => fabric_intervals(range),
        Flavor::Maven => maven_intervals(range),
    };
    match ivs {
        Some(v) => v.iter().map(fmt_interval).collect::<Vec<_>>().join(" ou "),
        None => range.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Flavor::{Fabric, Maven};

    fn m(r: &str, v: &str, f: Flavor) -> Option<bool> {
        matches(r, v, f)
    }

    #[test]
    fn comparaison_de_versions() {
        assert_eq!(compare("1.21.1", "1.21"), Some(Ordering::Greater));
        assert_eq!(compare("1.21", "1.21.0"), Some(Ordering::Equal));
        assert_eq!(compare("1.9", "1.10"), Some(Ordering::Less));
        assert_eq!(compare("21.1.250", "21.1.9"), Some(Ordering::Greater));
        assert_eq!(compare("1.0.0-beta.2", "1.0.0"), Some(Ordering::Less));
        assert_eq!(compare("1.0.0-beta.2", "1.0.0-beta.10"), Some(Ordering::Less));
        assert_eq!(compare("1.0.0-alpha", "1.0.0-beta"), Some(Ordering::Less));
        assert_eq!(compare("1.0.0-rc1", "1.0.0-rc2"), Some(Ordering::Less));
        assert_eq!(compare("1.0.0+build.5", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("1.21-pre1", "1.21"), Some(Ordering::Less));
        assert_eq!(compare("1.21-pre1", "1.20.6"), Some(Ordering::Greater));
        assert_eq!(compare("24w14a", "24w13a"), Some(Ordering::Greater));
        assert_eq!(compare("24w14a", "1.21"), None);
        assert_eq!(compare("abc", "1.0"), None);
        assert_eq!(compare("${file.jarVersion}", "1.0"), None);
    }

    #[test]
    fn fabric_operateurs() {
        assert_eq!(m(">=1.0", "1.0", Fabric), Some(true));
        assert_eq!(m(">=1.0", "0.9", Fabric), Some(false));
        assert_eq!(m(">1.0", "1.0", Fabric), Some(false));
        assert_eq!(m("<=1.0", "1.0", Fabric), Some(true));
        assert_eq!(m("<1.0", "1.0", Fabric), Some(false));
        assert_eq!(m("=1.2.3", "1.2.3", Fabric), Some(true));
        assert_eq!(m("1.2.3", "1.2.4", Fabric), Some(false));
        assert_eq!(m("*", "whatever", Fabric), Some(true));
        assert_eq!(m(">=1.20 <1.21", "1.20.4", Fabric), Some(true));
        assert_eq!(m(">=1.20 <1.21", "1.21", Fabric), Some(false));
    }

    #[test]
    fn fabric_tilde_caret_wildcard_ou() {
        assert_eq!(m("~1.20.1", "1.20.4", Fabric), Some(true));
        assert_eq!(m("~1.20.1", "1.21", Fabric), Some(false));
        assert_eq!(m("~1.20.1", "1.20.0", Fabric), Some(false));
        assert_eq!(m("^1.2", "1.9.1", Fabric), Some(true));
        assert_eq!(m("^1.2", "2.0", Fabric), Some(false));
        assert_eq!(m("1.20.x", "1.20.6", Fabric), Some(true));
        assert_eq!(m("1.20.*", "1.21.1", Fabric), Some(false));
        assert_eq!(m("1.x", "1.5", Fabric), Some(true));
        assert_eq!(m("1.20.1 || 1.20.4", "1.20.4", Fabric), Some(true));
        assert_eq!(m("1.20.1 || 1.20.4", "1.20.2", Fabric), Some(false));
        assert_eq!(m(">=1.21- <1.22-", "1.21.1", Fabric), Some(true));
    }

    #[test]
    fn fabric_indecidable() {
        assert_eq!(m(">=1.0", "24w14a", Fabric), None);
        assert_eq!(m(">=1.0", "n/a", Fabric), None);
        assert_eq!(m(">=truc", "1.0", Fabric), None);
        assert_eq!(m("1.20.1 || machin", "1.21", Fabric), None);
        assert_eq!(m("1.20.1 || machin", "1.20.1", Fabric), Some(true));
    }

    #[test]
    fn maven_intervalles() {
        assert_eq!(m("[1.20.1,1.21)", "1.20.1", Maven), Some(true));
        assert_eq!(m("[1.20.1,1.21)", "1.21", Maven), Some(false));
        assert_eq!(m("[1.20.1,1.21)", "1.20.6", Maven), Some(true));
        assert_eq!(m("(,1.21]", "1.21", Maven), Some(true));
        assert_eq!(m("(,1.21]", "1.21.1", Maven), Some(false));
        assert_eq!(m("[1.0,)", "5.0", Maven), Some(true));
        assert_eq!(m("[1.0,)", "0.5", Maven), Some(false));
        assert_eq!(m("(1.0,)", "1.0", Maven), Some(false));
        assert_eq!(m("[1.20.1]", "1.20.1", Maven), Some(true));
        assert_eq!(m("[1.20.1]", "1.20.2", Maven), Some(false));
        assert_eq!(m("[1.0,2.0),[3.0,4.0)", "3.5", Maven), Some(true));
        assert_eq!(m("[1.0,2.0),[3.0,4.0)", "2.5", Maven), Some(false));
        assert_eq!(m("[21.1,)", "21.1.250", Maven), Some(true));
    }

    #[test]
    fn maven_version_nue_et_indecidable() {
        assert_eq!(m("1.0", "1.5", Maven), Some(true));
        assert_eq!(m("1.0", "0.5", Maven), Some(false));
        assert_eq!(m("", "0.5", Maven), Some(true));
        assert_eq!(m("[1.0,machin)", "1.5", Maven), None);
        assert_eq!(m("[1.0,2.0", "1.5", Maven), None);
        assert_eq!(m("[1.0,)", "${x}", Maven), None);
        assert_eq!(m("[1.0,)", "24w14a", Maven), None);
    }

    #[test]
    fn format_mc_prefixe_indecidable_et_famille() {
        assert_eq!(m("[26.1.2-12.5.5,)", "12.6.12", Maven), None);
        assert_eq!(m("[26.1.2-12.5.5,)", "26.1.2-12.6.1", Maven), Some(true));
        assert!(same_family("[1.21,1.21.1)", "1.21.1"));
        assert!(same_family("[1.20.1,1.21)", "1.21.4"));
        assert!(!same_family("[1.20,1.20.2)", "1.21.1"));
    }

    #[test]
    fn texte_lisible() {
        assert_eq!(describe(">=1.0", Fabric), "1.0 ou plus récent");
        assert_eq!(describe("[1.0,)", Maven), "1.0 ou plus récent");
        assert_eq!(describe("1.0", Maven), "1.0 ou plus récent");
        assert_eq!(describe("[1.20.1,1.21)", Maven), "entre 1.20.1 et 1.21 (exclu)");
        assert_eq!(describe("[1.20.1]", Maven), "1.20.1");
        assert_eq!(describe("1.20.1", Fabric), "1.20.1");
        assert_eq!(describe(">=1.20 <1.21", Fabric), "entre 1.20 et 1.21 (exclu)");
        assert_eq!(describe("1.20.1 || 1.20.4", Fabric), "1.20.1 ou 1.20.4");
        assert_eq!(describe("~1.20.1", Fabric), "~1.20.1");
        assert_eq!(describe("<2", Fabric), "plus ancien que 2");
    }
}
