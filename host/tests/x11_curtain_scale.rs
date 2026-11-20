//! Scene X11 : le rideau suit l'echelle d'affichage du launcher (`GameHost::set_scale`), pas seulement `GDK_SCALE`. Vrai serveur X
//! (`xvfb-run`), vrai rendu logiciel, vrai `PutImage` : on LIT l'image du rideau (`GetImage`) avant et apres `set_scale`, sans
//! aucun recalage entre les deux (le rectangle du rideau ne bouge pas : seul l'appel explicite peut le redessiner).
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;
use x11rb::protocol::xproto::*;

const T: Duration = Duration::from_secs(15);

/// Image du rideau : (largeur, hauteur, octets BGRA).
fn grab(win: u32) -> (usize, usize, Vec<u8>) {
    let (c, _) = x();
    let g = c.get_geometry(win).unwrap().reply().unwrap();
    let img = c.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, g.width, g.height, !0).unwrap().reply().unwrap();
    assert_eq!(img.data.len(), g.width as usize * g.height as usize * 4, "profondeur inattendue ({})", img.depth);
    (g.width as usize, g.height as usize, img.data)
}

/// Hauteur (en pixels) de la plage de lignes dont un pixel s'ecarte nettement du fond du rideau (la couleur la plus frequente) :
/// texte, logo, barre. Elle grandit avec l'echelle. 0 tant que le rideau n'est pas dessine (fenetre encore transparente).
fn content_height(win: u32) -> usize {
    let (w, h, d) = grab(win);
    let px = |i: usize| u32::from_le_bytes([d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]]);
    let mut freq = std::collections::HashMap::new();
    for i in 0..w * h {
        *freq.entry(px(i)).or_insert(0usize) += 1;
    }
    let bg = *freq.iter().max_by_key(|(_, n)| **n).unwrap().0;
    if bg >> 24 == 0 {
        return 0; // fond transparent : rien n'est encore dessine
    }
    let far = |p: u32| (0..3).any(|k| ((p >> (8 * k)) & 255).abs_diff((bg >> (8 * k)) & 255) > 60);
    let rows: Vec<usize> = (0..h).filter(|y| (0..w).any(|x| far(px(y * w + x)))).collect();
    match (rows.first(), rows.last()) {
        (Some(a), Some(b)) => b - a + 1,
        _ => 0,
    }
}

/// Attend que le rideau soit dessine et stable (animation d'entree finie) : deux lectures identiques a la suite, non nulles.
fn settled(win: u32) -> usize {
    let mut last = usize::MAX;
    let mut now = content_height(win);
    let start = std::time::Instant::now();
    while now == 0 || now != last {
        assert!(start.elapsed() < T, "le rideau ne se stabilise pas (dernier contenu : {now} px)");
        last = now;
        pump_for(Duration::from_millis(400));
        now = content_height(win);
    }
    now
}

#[test]
fn le_rideau_suit_l_echelle_du_launcher() {
    if !available() {
        return;
    }
    std::env::remove_var("GDK_SCALE");
    let parent = Parent::new();
    let host = GameHost::embed(parent.id()).expect("embed");
    let (cw, ch) = parent.client_size();
    host.relayout(0, 0, cw, ch);
    let curtain = curtain();
    host.begin("Modpack de test", "Lancement", None, None);
    wait_until("rideau ferme", T, || visible(curtain));
    let rect_before = rect(curtain);

    let h1 = settled(curtain);
    assert!(h1 > 0, "le rideau n'affiche rien");
    println!("echelle 1 : contenu sur {h1} px");

    host.set_scale(2.0);
    let h2 = settled(curtain);
    println!("echelle 2 : contenu sur {h2} px");
    assert!(h2 > h1, "le rideau n'a pas suivi set_scale(2.0) : {h1} -> {h2} px");
    assert_eq!(rect(curtain), rect_before, "le rectangle du rideau n'a pas bouge : c'est bien le redessin explicite");

    // le logo respire un peu : on compare a mi-chemin entre les deux echelles, pas au pixel pres
    let mid = (h1 + h2) / 2;

    // valeur aberrante : ramenee a 1 (jamais de rideau vide ni enorme)
    host.set_scale(f32::NAN);
    let n = settled(curtain);
    assert!(n < mid, "NaN doit ramener a l'echelle 1 : {n} px (1x : {h1}, 2x : {h2})");
    host.set_scale(2.0);
    let n = settled(curtain);
    assert!(n > mid, "retour a l'echelle 2 : {n} px (1x : {h1}, 2x : {h2})");
    host.set_scale(1.0);
    let n = settled(curtain);
    assert!(n < mid, "retour a l'echelle 1 : {n} px (1x : {h1}, 2x : {h2})");
}
