//! Prototype : fenetre hote Cubelink qui enchaine plusieurs jeux sans qu'une fenetre de jeu autonome ne soit jamais visible.
//!
//! Usage : host-proto --out <dossier> [--delay-ms 6000] <spec1.json> [<spec2.json> ...]
//! Chaque spec est ecrite par le launcher avec `CUBELINK_DUMP_LAUNCH=<fichier>` : {program, args, cwd, env}.
//! Aucune entree souris/clavier n'est simulee. Sortie : report.txt, shot-*.bmp (hote, ~1 par seconde), verdict en code de sortie.

#[cfg(not(windows))]
fn main() {
    eprintln!("host-proto : Windows seulement pour l'instant");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    use cubelink_host::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let mut out = std::path::PathBuf::from(".");
    let mut delay = 2500u64;
    let mut keep = false;
    let mut specs = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().unwrap().into(),
            "--keep" => keep = true,
            "--delay-ms" => delay = it.next().unwrap().parse().unwrap(),
            _ => specs.push(a),
        }
    }
    std::fs::create_dir_all(&out).unwrap();

    fn quote(a: &str) -> String {
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            return a.to_string();
        }
        let mut s = String::from("\"");
        let mut bs = 0;
        for c in a.chars() {
            match c {
                '\\' => bs += 1,
                '"' => {
                    s.push_str(&"\\".repeat(bs * 2 + 1));
                    s.push('"');
                    bs = 0;
                }
                _ => {
                    s.push_str(&"\\".repeat(bs));
                    bs = 0;
                    s.push(c);
                }
            }
        }
        s.push_str(&"\\".repeat(bs * 2));
        s.push('"');
        s
    }

    let games: Vec<GameSpec> = specs
        .iter()
        .map(|p| {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
            let args: Vec<String> = v["args"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
            let env = v["env"].as_array().map(|a| a.iter().map(|e| (e[0].as_str().unwrap().to_string(), e[1].as_str().unwrap().to_string())).collect()).unwrap_or_default();
            GameSpec {
                label: std::path::Path::new(p).file_stem().unwrap().to_string_lossy().to_string(),
                title: v["title"].as_str().map_or_else(|| std::path::Path::new(p).file_stem().unwrap().to_string_lossy().to_string(), |t| t.to_string()),
                mods_dir: v["cwd"].as_str().map(|c| std::path::Path::new(c).join("mods")),
                profile_path: Some(out.join(format!("profile-{}.json", std::path::Path::new(p).file_stem().unwrap().to_string_lossy()))),
                log_path: Some(out.join(format!("game-{}.log", std::path::Path::new(p).file_stem().unwrap().to_string_lossy()))),
                program: v["program"].as_str().unwrap().into(),
                raw_args: args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "),
                cwd: v["cwd"].as_str().map(Into::into),
                env,
            }
        })
        .collect();

    let host = Arc::new(GameHost::start(HostConfig { title: "Cubelink".into(), width: 1280, height: 760 }).expect("hote"));
    let t0 = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    // Sonde 1 : toute fenetre de premier niveau visible et a l'ecran d'un jeu = une fenetre autonome vue par le joueur.
    let leaks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sampler = {
        let (h, stop, leaks) = (host.clone(), stop.clone(), leaks.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                for w in h.toplevel_windows() {
                    if w.visible && w.x > -20000 && w.w > 1 {
                        let line = format!("+{} ms : fenetre autonome visible {:?} {:?} a ({},{}) {}x{}", t0.elapsed().as_millis(), w.class, w.title, w.x, w.y, w.w, w.h);
                        let mut l = leaks.lock().unwrap();
                        if l.len() < 40 {
                            l.push(line);
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    // Sonde 2 : captures de l'hote (ce que voit le joueur dans la fenetre) environ une par seconde.
    let shooter = {
        let (h, stop, out) = (host.clone(), stop.clone(), out.clone());
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::SeqCst) {
                if let Some((w, hh, px)) = h.screenshot() {
                    let _ = write_bmp(&out.join(format!("shot-{:03}-{:05}.bmp", n, t0.elapsed().as_millis())), w, hh, &px);
                    n += 1;
                }
                std::thread::sleep(Duration::from_millis(900));
            }
        })
    };

    let mut report = Vec::new();
    let mut ok = true;
    for (i, g) in games.iter().enumerate() {
        let t = Instant::now();
        match host.switch_to(g, &Ready::Auto { quiet: Duration::from_millis(delay), max: Duration::from_secs(240) }) {
            Ok(r) => report.push(format!("jeu {} {:?} : pid {} spawn {} ms, fenetre adoptee et affichee {} ms, pret (rideau retire) {} ms, remplacait un jeu : {}, fermeture propre de l'ancien : {:?}", i + 1, g.label, r.pid, r.spawn_ms, r.window_ms, r.ready_ms, r.had_game, r.closed)),
            Err(e) => {
                ok = false;
                report.push(format!("jeu {} {:?} : ECHEC {e} (apres {} ms)", i + 1, g.label, t.elapsed().as_millis()));
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(2)); // laisse le jeu pret quelques secondes (captures)
    }
    if keep {
        // mode manuel : la fenetre reste ouverte (le joueur essaie), on enchaine les jeux en boucle toutes les 20 s
        let mut i = 0usize;
        while host.is_open() && !games.is_empty() {
            for _ in 0..200 {
                if !host.is_open() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !host.is_open() {
                break;
            }
            i += 1;
            let g = &games[i % games.len()];
            match host.switch_to(g, &Ready::Auto { quiet: Duration::from_millis(delay), max: Duration::from_secs(240) }) {
                Ok(_) => {}
                Err(e) => eprintln!("changement impossible : {e}"),
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    let _ = sampler.join();
    let _ = shooter.join();
    report.push("--- journal d'adoption ---".into());
    report.extend(host.adopt_log());
    let leaks = leaks.lock().unwrap().clone();
    report.push(format!("--- fenetres autonomes visibles : {} ---", leaks.len()));
    report.extend(leaks.iter().cloned());
    if !leaks.is_empty() {
        ok = false;
    }
    report.push(if ok { "VERDICT : OK".into() } else { "VERDICT : ECHEC".into() });
    std::fs::write(out.join("report.txt"), report.join("\n")).unwrap();
    println!("{}", report.join("\n"));
    host.close();
    std::thread::sleep(Duration::from_millis(500));
    std::process::exit(if ok { 0 } else { 1 });
}
