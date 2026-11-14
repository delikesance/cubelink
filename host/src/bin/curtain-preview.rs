//! Apercu du rideau en PNG (sans fenetre, sans jeu).
//!   `curtain-preview <dossier>`          : les etats-types en PNG.
//!   `curtain-preview matrix <dossier>`   : une matrice d'etats (cover / carte / barre / calm) en PNG, pour comparer avant/apres.
//!   `curtain-preview bench [N]`          : rend N images d'une animation et affiche ms/image (a lancer en --release).
use cubelink_host::curtain::*;

const NOTE: &str = "Ton monde est enregistré avant la fermeture : rien n'est perdu.";

fn model<'a>(sentence: &'a str, note: Option<&'a str>, fraction: f32, cover: f32, card: f32, t: f32, calm: bool) -> Model<'a> {
    Model { title: "Fabulously Optimized", caption: cubelink_host::flow::CAPTION_SWITCH, sentence, note, fraction, cover, card, t, calm }
}

fn composite(top: &tiny_skia::Pixmap, w: u32, h: u32) -> tiny_skia::Pixmap {
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(70, 140, 80, 255)); // « jeu » derriere le rideau
    pm.draw_pixmap(0, 0, top.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    pm
}

fn matrix(out: &std::path::Path) {
    let mut c = Curtain::new();
    let (w, h) = (1264u32, 721u32);
    let mut n = 0;
    // (cover, card, fraction, t, calm, note)
    let covers = [0.0f32, 0.25, 0.5, 1.0];
    let cards = [0.0f32, 0.5, 1.0];
    let fracs = [0.0f32, 0.5, 1.0];
    for &cover in &covers {
        for &card in &cards {
            for &f in &fracs {
                for &(t, calm) in &[(0.0f32, false), (2.0, false), (3.7, false), (2.0, true)] {
                    let note = if n % 2 == 0 { Some(NOTE) } else { None };
                    let m = model("Chargement des mods…", note, f, cover, card, t, calm);
                    let top = c.render(&m, w, h, 1.0);
                    composite(&top, w, h).save_png(out.join(format!("m{n:03}.png"))).unwrap();
                    n += 1;
                }
            }
        }
    }
    // changement de taille / echelle puis retour (invalidation de cache)
    for &(w2, h2, s) in &[(800u32, 480u32, 1.0f32), (1920, 1080, 1.5), (1264, 721, 1.0)] {
        let m = model("Mise en place…", Some(NOTE), 0.37, 1.0, 1.0, 1.3, false);
        let top = c.render(&m, w2, h2, s);
        composite(&top, w2, h2).save_png(out.join(format!("m{n:03}.png"))).unwrap();
        n += 1;
    }
    // phrase / titre changeants a la suite (cache de texte)
    for (i, s) in ["Enregistrement de ton monde…", "Chargement des mods…", "C'est prêt !"].iter().enumerate() {
        let m = model(s, None, 0.3 * (i + 1) as f32, 1.0, 1.0, 0.5 * i as f32, false);
        let top = c.render(&m, w, h, 1.0);
        composite(&top, w, h).save_png(out.join(format!("m{n:03}.png"))).unwrap();
        n += 1;
    }
    println!("matrice : {n} images dans {}", out.display());
}

fn bench(frames: u32) {
    let (w, h) = (1264u32, 721u32);
    let mut c = Curtain::new();
    let run = |c: &mut Curtain, fast: bool, label: &str, f: &dyn Fn(u32) -> (f32, f32, f32, f32)| {
        // echauffement
        for i in 0..10 {
            let (fr, cv, cd, t) = f(i);
            let _ = c.render(&model("Chargement des mods…", Some(NOTE), fr, cv, cd, t, false), w, h, 1.0);
        }
        let t0 = std::time::Instant::now();
        let mut buf: Vec<u8> = Vec::new();
        let mut sink = 0u64;
        for i in 0..frames {
            let (fr, cv, cd, t) = f(i);
            let m = model("Chargement des mods…", Some(NOTE), fr, cv, cd, t, false);
            if fast {
                let pm = c.render_ref(&m, w, h, 1.0);
                to_bgra_into(pm, &mut buf);
                sink += buf[(i as usize * 7919) % buf.len()] as u64;
            } else {
                let pm = c.render(&m, w, h, 1.0);
                let b = to_bgra(&pm);
                sink += b[(i as usize * 7919) % b.len()] as u64;
            }
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        println!("{label:<28} {ms:7.3} ms/image ({})   [{sink}]", if fast { "render_ref + to_bgra_into" } else { "render + to_bgra" });
    };
    run(&mut c, false, "ferme, barre qui avance", &|i| (i as f32 / frames as f32, 1.0, 1.0, i as f32 / 60.0));
    run(&mut c, false, "ferme, barre fixe 50 %", &|i| (0.5, 1.0, 1.0, i as f32 / 60.0));
    run(&mut c, false, "panneaux qui se ferment", &|i| (0.0, i as f32 / frames as f32, 0.0, i as f32 / 60.0));
    run(&mut c, false, "ouverture (carte qui fond)", &|i| (1.0, 1.0 - 0.5 * i as f32 / frames as f32, 1.0 - i as f32 / frames as f32, i as f32 / 60.0));
    run(&mut c, true, "ferme, barre qui avance", &|i| (i as f32 / frames as f32, 1.0, 1.0, i as f32 / 60.0));
    run(&mut c, true, "ferme, barre fixe 50 %", &|i| (0.5, 1.0, 1.0, i as f32 / 60.0));
    run(&mut c, true, "panneaux qui se ferment", &|i| (0.0, i as f32 / frames as f32, 0.0, i as f32 / 60.0));
    run(&mut c, true, "ouverture (carte qui fond)", &|i| (1.0, 1.0 - 0.5 * i as f32 / frames as f32, 1.0 - i as f32 / frames as f32, i as f32 / 60.0));
}

fn main() {
    let a1 = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if a1 == "bench" {
        bench(std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(300));
        return;
    }
    if a1 == "matrix" {
        let out = std::path::PathBuf::from(std::env::args().nth(2).unwrap_or_else(|| ".".into()));
        std::fs::create_dir_all(&out).unwrap();
        matrix(&out);
        return;
    }
    let out = std::path::PathBuf::from(a1);
    std::fs::create_dir_all(&out).unwrap();
    let mut c = Curtain::new();

    let steps = [
        ("1-ferme", "Changement de modpack", "Enregistrement de ton monde…", Some("Ton monde est enregistré avant la fermeture : rien n'est perdu."), 0.30, 1.0, 1.0),
        ("2-ouvre", "Changement de modpack", "Chargement des mods…", None, 0.72, 1.0, 1.0),
        ("3-lent", "Lancement", "Mise en place des textures et des sons…", Some("Ça prend un peu plus de temps que d'habitude, tout va bien."), 0.88, 1.0, 1.0),
        ("4-panneaux-40", "Changement de modpack", "", None, 0.0, 0.40, 0.0),
        ("5-panneaux-85", "Changement de modpack", "", None, 0.0, 0.85, 0.0),
        ("6-ouverture", "Changement de modpack", "C'est prêt !", None, 1.0, 0.55, 0.0),
    ];
    for (name, cap, sentence, note, f, cover, card) in steps {
        let m = Model { title: "Fabulously Optimized", caption: cap, sentence, note, fraction: f, cover, card, t: 2.0, calm: false };
        let top = c.render(&m, 1264, 721, 1.0);
        composite(&top, 1264, 721).save_png(out.join(format!("{name}.png"))).unwrap();
    }
    let tb = c.render_titlebar("Cubelink · Fabulously Optimized", 1264, 36, 1.0, Some(2), false);
    tb.save_png(out.join("7-titlebar.png")).unwrap();
    println!("ok : {}", out.display());
}
