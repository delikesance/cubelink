//! Rideau de chargement GRAND PUBLIC, dans la D.A. du launcher (tokens.css) : le logo, le nom du modpack, UNE barre qui dit
//! vraiment ou on en est, une phrase simple. Aucun detail technique. Rendu logiciel sans systeme (testable en PNG).
//!
//! Choix UX : barre determinee et monotone ; la phrase explique l'attente ; mise en page stable (emplacements reserves) ;
//! deux mouvements discrets seulement (le morceau du logo flotte, un reflet traverse la barre) ; `calm` les fige.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode, Stroke,
    Transform,
};

// ---- tokens.css
const BG: [u8; 3] = [0x13, 0x11, 0x16];
const SURFACE2: [u8; 3] = [0x24, 0x20, 0x29];
const LINE: [u8; 3] = [0x33, 0x2E, 0x3A];
const TEXT: [u8; 3] = [0xF4, 0xF1, 0xEC];
const MUTED: [u8; 3] = [0xB0, 0xA8, 0xB4];
const ACCENT: [u8; 3] = [0x4F, 0xC3, 0xE8];
const ACCENT_HOVER: [u8; 3] = [0x6F, 0xD3, 0xF0];
const ACCENT_EDGE: [u8; 3] = [0x1E, 0x7F, 0xA3];

static INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
static INTER_SEMI: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
static GROTESK: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf");

// Logo p4-bloc (src/screens/Login/Logo.tsx, variante normale), repere 256.
const BODY: &str = "M24 104Q24 84 44 84L104 84Q118 84 118 98L118 118Q118 140 140 140L160 140Q174 140 174 154L174 214Q174 234 154 234L44 234Q24 234 24 214Z";
const PIECE: &str = "M172 56Q172 36 192 36L208 36Q228 36 228 56L228 72Q228 92 208 92L192 92Q172 92 172 72Z";
const BODY_SLAB: &str = "M24 124Q24 104 44 104L104 104Q118 104 118 118L118 138Q118 160 140 160L160 160Q174 160 174 174L174 234Q174 254 154 254L44 254Q24 254 24 234Z";
const PIECE_SLAB: &str = "M172 76Q172 56 192 56L208 56Q228 56 228 76L228 92Q228 112 208 112L192 112Q172 112 172 92Z";

const BODY_SLAB_SMALL: &str = "M24 128Q24 108 44 108L104 108Q118 108 118 122L118 142Q118 164 140 164L160 164Q174 164 174 178L174 238Q174 258 154 258L44 258Q24 258 24 238Z";
const PIECE_SMALL: &str = "M168 48Q168 28 188 28L212 28Q232 28 232 48L232 72Q232 92 212 92L188 92Q168 92 168 72Z";
const PIECE_SLAB_SMALL: &str = "M168 72Q168 52 188 52L212 52Q232 52 232 72L232 96Q232 116 212 116L188 116Q168 116 168 96Z";

/// Tout ce que montre le rideau.
pub struct Model<'a> {
    /// Nom du modpack qui s'ouvre.
    pub title: &'a str,
    /// Petite legende en capitales (« Changement de modpack », « Lancement »).
    pub caption: &'a str,
    /// Une phrase simple.
    pub sentence: &'a str,
    /// Ligne rassurante (seulement si ca traine) ; l'emplacement est toujours reserve.
    pub note: Option<&'a str>,
    /// Avancement 0..1 (monotone).
    pub fraction: f32,
    /// Fermeture du rideau : 0 = ouvert (le jeu est visible), 1 = ferme (deux panneaux qui se rejoignent au centre).
    pub cover: f32,
    /// Opacite du contenu (logo, titre, barre) : apparait une fois ferme, disparait avant l'ouverture.
    pub card: f32,
    /// Secondes depuis l'apparition du rideau (animations).
    pub t: f32,
    /// Mouvement reduit : pas de flottement ni de reflet.
    pub calm: bool,
}

/// Chemins du logo, analyses une seule fois.
struct LogoPaths {
    body: Option<tiny_skia::Path>,
    piece: Option<tiny_skia::Path>,
    body_slab: Option<tiny_skia::Path>,
    piece_slab: Option<tiny_skia::Path>,
    body_slab_small: Option<tiny_skia::Path>,
    piece_small: Option<tiny_skia::Path>,
    piece_slab_small: Option<tiny_skia::Path>,
}

/// Ce qui determine la couche statique d'une image (tout sauf la barre, le pourcentage et le morceau du logo).
#[derive(PartialEq)]
struct BaseKey {
    w: u32,
    h: u32,
    scale: u32,
    card: u32,
    title: String,
    caption: String,
    sentence: String,
    note: Option<String>,
}

/// Rectangle de pixels [x0, x1) x [y0, y1).
#[derive(Clone, Copy)]
struct Rect4 {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// Mise en page d'une image (en pixels).
struct Geo {
    s: f32,
    a: f32,
    x0: f32,
    bw: f32,
    bh: f32,
    bar_y: f32,
    line_y: f32,
    logo_x: f32,
    logo_y: f32,
    logo_w: f32,
    piece_rect: Rect4,
    bar_rect: Rect4,
    pct_rect: Rect4,
}

pub struct Curtain {
    inter: FontRef<'static>,
    semi: FontRef<'static>,
    grotesk: FontRef<'static>,
    bg: Option<Pixmap>,
    bg_key: (u32, u32),
    paths: LogoPaths,
    /// Couche statique (fond, corps du logo, textes, piste de la barre) et image courante (= couche + parties animees).
    base: Option<Pixmap>,
    base_key: Option<BaseKey>,
    scene: Option<Pixmap>,
    /// Image avec panneaux (cover < 1), reutilisee.
    out: Option<Pixmap>,
    mask: Option<Mask>,
}

impl Default for Curtain {
    fn default() -> Self {
        Self::new()
    }
}

fn col(c: [u8; 3], a: f32) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.5522847 * r;
    let mut b = PathBuilder::new();
    b.move_to(x + r, y);
    b.line_to(x + w - r, y);
    b.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    b.line_to(x + w, y + h - r);
    b.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    b.line_to(x + r, y + h);
    b.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    b.line_to(x, y + r);
    b.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    b.close();
    b.finish()
}

fn fill_rr(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
    if let Some(p) = rrect(x, y, w, h, r) {
        pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}

#[allow(clippy::too_many_arguments)] // primitive de dessin : rectangle + rayon + trait + couleur
fn stroke_rr(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: Color) {
    if let Some(p) = rrect(x, y, w, h, r) {
        let s = Stroke { width, ..Stroke::default() };
        pm.stroke_path(&p, &paint(c), &s, Transform::identity(), None);
    }
}

/// Chemin SVG minimal (M, L, Q, Z absolus) : suffit au logo.
fn svg_path(d: &str) -> Option<tiny_skia::Path> {
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in d.chars() {
        if ch.is_ascii_alphabetic() {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            toks.push(ch.to_string());
        } else if ch == ' ' || ch == ',' {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f32>().ok());
    let mut b = PathBuilder::new();
    let mut i = 0;
    while i < toks.len() {
        match toks[i].as_str() {
            "M" => {
                b.move_to(num(i + 1)?, num(i + 2)?);
                i += 3;
            }
            "L" => {
                b.line_to(num(i + 1)?, num(i + 2)?);
                i += 3;
            }
            "Q" => {
                b.quad_to(num(i + 1)?, num(i + 2)?, num(i + 3)?, num(i + 4)?);
                i += 5;
            }
            "Z" => {
                b.close();
                i += 1;
            }
            _ => return None,
        }
    }
    b.finish()
}

impl Curtain {
    pub fn new() -> Curtain {
        Curtain {
            inter: FontRef::try_from_slice(INTER).expect("Inter"),
            semi: FontRef::try_from_slice(INTER_SEMI).expect("Inter SemiBold"),
            grotesk: FontRef::try_from_slice(GROTESK).expect("Space Grotesk"),
            bg: None,
            bg_key: (0, 0),
            paths: LogoPaths {
                body: svg_path(BODY),
                piece: svg_path(PIECE),
                body_slab: svg_path(BODY_SLAB),
                piece_slab: svg_path(PIECE_SLAB),
                body_slab_small: svg_path(BODY_SLAB_SMALL),
                piece_small: svg_path(PIECE_SMALL),
                piece_slab_small: svg_path(PIECE_SLAB_SMALL),
            },
            base: None,
            base_key: None,
            scene: None,
            out: None,
            mask: None,
        }
    }

    fn font(&self, f: u8) -> &FontRef<'static> {
        match f {
            0 => &self.inter,
            1 => &self.semi,
            _ => &self.grotesk,
        }
    }

    fn measure(&self, f: u8, size: f32, tracking: f32, s: &str) -> f32 {
        let sc = self.font(f).as_scaled(PxScale::from(size));
        let mut w = 0.0;
        let mut prev = None;
        for ch in s.chars() {
            let g = sc.glyph_id(ch);
            if let Some(p) = prev {
                w += sc.kern(p, g);
            }
            w += sc.h_advance(g) + tracking;
            prev = Some(g);
        }
        w
    }

    fn fit(&self, f: u8, size: f32, tracking: f32, s: &str, max: f32) -> String {
        if self.measure(f, size, tracking, s) <= max {
            return s.to_string();
        }
        let mut out: String = s.to_string();
        while !out.is_empty() && self.measure(f, size, tracking, &format!("{out}…")) > max {
            out.pop();
        }
        format!("{out}…")
    }

    /// Dessine un texte (`y` = haut de la ligne).
    #[allow(clippy::too_many_arguments)]
    fn text(&self, pm: &mut Pixmap, f: u8, size: f32, tracking: f32, x: f32, y: f32, c: [u8; 3], alpha: f32, s: &str) {
        let font = self.font(f);
        let sc = font.as_scaled(PxScale::from(size));
        let base = y + sc.ascent();
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        let data = pm.data_mut();
        let mut caret = x;
        let mut prev = None;
        for ch in s.chars() {
            let gid = sc.glyph_id(ch);
            if let Some(p) = prev {
                caret += sc.kern(p, gid);
            }
            let g = gid.with_scale_and_position(PxScale::from(size), point(caret, base));
            if let Some(og) = font.outline_glyph(g) {
                let b = og.px_bounds();
                let (bx, by) = (b.min.x as i32, b.min.y as i32);
                og.draw(|gx, gy, cov| {
                    let (px, py) = (bx + gx as i32, by + gy as i32);
                    if px < 0 || py < 0 || px >= w || py >= h {
                        return;
                    }
                    let a = (cov * alpha).clamp(0.0, 1.0);
                    let i = ((py * w + px) * 4) as usize;
                    for k in 0..3 {
                        let d = data[i + k] as f32;
                        data[i + k] = (d + (c[k] as f32 - d) * a) as u8;
                    }
                });
            }
            caret += sc.h_advance(gid) + tracking;
            prev = Some(gid);
        }
    }

    /// Fond des panneaux : `--bg` avec une lueur d'accent tres douce derriere le logo.
    fn background(&mut self, w: u32, h: u32) -> &Pixmap {
        let key = (w, h);
        if self.bg.is_none() || self.bg_key != key {
            let mut pm = Pixmap::new(w, h).expect("pixmap");
            pm.fill(col(BG, 1.0));
            let full = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap();
            let c = Point::from_xy(w as f32 / 2.0, h as f32 * 0.40);
            if let Some(sh) = RadialGradient::new(
                c,
                c,
                (w.max(h) as f32) * 0.55,
                vec![GradientStop::new(0.0, col(ACCENT, 0.10)), GradientStop::new(1.0, col(ACCENT, 0.0))],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                let p = Paint { shader: sh, ..Paint::default() };
                pm.fill_rect(full, &p, Transform::identity(), None);
            }
            self.bg = Some(pm);
            self.bg_key = key;
        }
        self.bg.as_ref().unwrap()
    }

    /// Transformations du logo : (variante small, corps, morceau, trait). `width` px de large, haut-gauche en (x, y) ;
    /// `lift` = decalage du morceau (unites du logo).
    fn logo_xf(x: f32, y: f32, width: f32, lift: (f32, f32)) -> (bool, Transform, Transform, Stroke) {
        let small = width < 48.0;
        let k = width / 232.0; // cadre serre du composant : viewBox « 16 10 232 248 »
        let base = Transform::from_row(k, 0.0, 0.0, k, x - 16.0 * k, y - 10.0 * k).pre_translate(6.0, -12.0);
        let piece_t = base.pre_translate(lift.0, lift.1);
        let stroke = Stroke { width: if small { 10.0 } else { 12.0 }, line_join: LineJoin::Round, line_cap: LineCap::Round, ..Stroke::default() };
        (small, base, piece_t, stroke)
    }

    fn draw_shape(pm: &mut Pixmap, p: &Option<tiny_skia::Path>, c: [u8; 3], alpha: f32, t: Transform, stroke: &Stroke) {
        if let Some(p) = p {
            let pt = paint(col(c, alpha));
            pm.fill_path(p, &pt, FillRule::Winding, t, None);
            pm.stroke_path(p, &pt, stroke, t, None);
        }
    }

    /// Le logo p4-bloc, `width` px de large, haut-gauche en (x, y) ; `lift` = decalage du morceau (unites du logo).
    fn logo(&self, pm: &mut Pixmap, x: f32, y: f32, width: f32, lift: (f32, f32), alpha: f32) {
        let (small, base, piece_t, stroke) = Self::logo_xf(x, y, width, lift);
        let p = &self.paths;
        let (bs, ps, pc) = if small { (&p.body_slab_small, &p.piece_slab_small, &p.piece_small) } else { (&p.body_slab, &p.piece_slab, &p.piece) };
        Self::draw_shape(pm, bs, ACCENT_EDGE, alpha, base, &stroke);
        Self::draw_shape(pm, ps, ACCENT_EDGE, alpha, piece_t, &stroke);
        Self::draw_shape(pm, &p.body, ACCENT, alpha, base, &stroke);
        Self::draw_shape(pm, pc, ACCENT_HOVER, alpha, piece_t, &stroke);
    }

    /// Partie fixe du logo (le corps), grande variante ; ne recouvre jamais le morceau.
    fn logo_body(&self, pm: &mut Pixmap, g: &Geo) {
        let (_, base, _, stroke) = Self::logo_xf(g.logo_x, g.logo_y, g.logo_w, (0.0, 0.0));
        Self::draw_shape(pm, &self.paths.body_slab, ACCENT_EDGE, g.a, base, &stroke);
        Self::draw_shape(pm, &self.paths.body, ACCENT, g.a, base, &stroke);
    }

    /// Partie animee du logo (le morceau flotte).
    fn logo_piece(&self, pm: &mut Pixmap, g: &Geo, lift: (f32, f32)) {
        let (_, _, piece_t, stroke) = Self::logo_xf(g.logo_x, g.logo_y, g.logo_w, lift);
        Self::draw_shape(pm, &self.paths.piece_slab, ACCENT_EDGE, g.a, piece_t, &stroke);
        Self::draw_shape(pm, &self.paths.piece, ACCENT_HOVER, g.a, piece_t, &stroke);
    }

    /// Mise en page (pixels) pour une taille / echelle / opacite de carte.
    fn geo(w: u32, h: u32, scale: f32, card: f32) -> Geo {
        let (logo_w, gap1, cap_h, gap2, title_h, gap3, bar_h, gap4, line_h, gap5, note_h) = (84.0, 26.0, 14.0, 8.0, 32.0, 30.0, 14.0, 16.0, 22.0, 6.0, 18.0);
        let logo_h = logo_w * 248.0 / 232.0;
        let content = logo_h + gap1 + cap_h + gap2 + title_h + gap3 + bar_h + gap4 + line_h + gap5 + note_h;
        let fit = ((h as f32 - 48.0) / (content * scale)).clamp(0.6, 1.0);
        let s = scale * fit;
        let a = card.clamp(0.0, 1.0);
        let e = a; // le contenu monte legerement en apparaissant
        let bw = (520.0 * s).min(w as f32 * 0.78).max(220.0);
        let x0 = (w as f32 - bw) / 2.0;
        let y = (h as f32 - content * s) / 2.0 - h as f32 * 0.02 + (1.0 - e) * 10.0 * s;
        let logo_x = (w as f32 - logo_w * s) / 2.0;
        let bar_y = y + (logo_h + gap1 + cap_h + gap2 + title_h + gap3) * s;
        let line_y = bar_y + (bar_h + gap4) * s;
        let bh = bar_h * s;
        let (wi, hi) = (w as i64, h as i64);
        let clamp = |x0: f32, y0: f32, x1: f32, y1: f32| Rect4 {
            x0: (x0.floor() as i64).clamp(0, wi) as usize,
            y0: (y0.floor() as i64).clamp(0, hi) as usize,
            x1: (x1.ceil() as i64).clamp(0, wi) as usize,
            y1: (y1.ceil() as i64).clamp(0, hi) as usize,
        };
        // le morceau du logo : unites 155..250 x 0..140, transformees comme dans `logo_xf` (marge large pour le flottement et le trait)
        let k = logo_w * s / 232.0;
        let (ox, oy) = (logo_x - 16.0 * k + 6.0 * k, y - 10.0 * k - 12.0 * k);
        let piece_rect = clamp(ox + 155.0 * k - 2.0, oy - 2.0, ox + 250.0 * k + 2.0, oy + 140.0 * k + 2.0);
        let bar_rect = clamp(x0 - 3.0, bar_y - 3.0, x0 + bw + 3.0, bar_y + bh + 3.0 * s + 3.0);
        let pct_rect = clamp(x0 + bw - 70.0 * s, line_y - 2.0, x0 + bw + 4.0, line_y + line_h * s + 2.0);
        Geo { s, a, x0, bw, bh, bar_y, line_y, logo_x, logo_y: y, logo_w: logo_w * s, piece_rect, bar_rect, pct_rect }
    }

    /// Reconstruit la couche statique : fond, corps du logo, legende, titre, piste de la barre, phrase, note.
    fn build_base(&mut self, m: &Model, w: u32, h: u32, g: &Geo) {
        let mut base = match self.base.take() {
            Some(b) if b.width() == w && b.height() == h => b,
            _ => Pixmap::new(w, h).expect("pixmap"),
        };
        base.data_mut().copy_from_slice(self.background(w, h).data());
        let (s, a) = (g.s, g.a);
        if a > 0.0 {
            self.logo_body(&mut base, g);
            let mut y = g.logo_y + (84.0 * 248.0 / 232.0 + 26.0) * s;
            let cap = m.caption.to_uppercase();
            let cw = self.measure(2, 11.0 * s, 1.0 * s, &cap);
            self.text(&mut base, 2, 11.0 * s, 1.0 * s, (w as f32 - cw) / 2.0, y, MUTED, a, &cap);
            y += (14.0 + 8.0) * s;
            let title = self.fit(2, 26.0 * s, 0.2 * s, m.title, g.bw);
            let tw = self.measure(2, 26.0 * s, 0.2 * s, &title);
            self.text(&mut base, 2, 26.0 * s, 0.2 * s, (w as f32 - tw) / 2.0, y, TEXT, a, &title);
            // piste de la barre
            let (x0, bw, bh, by) = (g.x0, g.bw, g.bh, g.bar_y);
            fill_rr(&mut base, x0, by, bw, bh, bh / 2.0, col(SURFACE2, a));
            stroke_rr(&mut base, x0 + 0.5, by + 0.5, bw - 1.0, bh - 1.0, bh / 2.0, 1.0, col(LINE, a));
            // phrase a gauche
            let sentence = self.fit(1, 15.0 * s, 0.0, m.sentence, bw - 70.0 * s);
            self.text(&mut base, 1, 15.0 * s, 0.0, x0, g.line_y, TEXT, a, &sentence);
            // ligne rassurante (emplacement reserve : rien ne saute quand elle apparait)
            if let Some(n) = m.note {
                let n = self.fit(0, 13.0 * s, 0.0, n, bw);
                self.text(&mut base, 0, 13.0 * s, 0.0, x0, g.line_y + (22.0 + 6.0) * s, MUTED, a, &n);
            }
        }
        self.base = Some(base);
    }

    /// Dessine une image complete `w` x `h` ; `scale` = facteur d'echelle ecran (DPI). Copie le resultat ; preferer `render_ref`.
    pub fn render(&mut self, m: &Model, w: u32, h: u32, scale: f32) -> Pixmap {
        self.render_ref(m, w, h, scale).clone()
    }

    /// Comme `render` sans allocation : la couche statique est mise en cache entre images (seuls la barre, le pourcentage
    /// et le morceau du logo sont redessines) et l'image retournee est reutilisee (valide jusqu'au prochain appel).
    pub fn render_ref(&mut self, m: &Model, w: u32, h: u32, scale: f32) -> &Pixmap {
        let (w, h) = (w.max(1), h.max(1));
        let g = Self::geo(w, h, scale, m.card);
        let key = BaseKey {
            w,
            h,
            scale: scale.to_bits(),
            card: g.a.to_bits(),
            title: m.title.to_string(),
            caption: m.caption.to_string(),
            sentence: m.sentence.to_string(),
            note: m.note.map(str::to_string),
        };
        let fresh = self.base_key.as_ref() != Some(&key) || self.base.is_none() || self.scene.is_none();
        if fresh {
            self.build_base(m, w, h, &g);
            self.base_key = Some(key);
            let base = self.base.as_ref().unwrap();
            match self.scene.as_mut() {
                Some(sc) if sc.width() == w && sc.height() == h => sc.data_mut().copy_from_slice(base.data()),
                _ => self.scene = Some(base.clone()),
            }
        }
        let mut scene = self.scene.take().unwrap();
        let base = self.base.take().unwrap();
        let bw = w as usize;
        let f = m.fraction.clamp(0.0, 1.0);
        if g.a > 0.0 {
            if !fresh {
                for r in [g.piece_rect, g.bar_rect, g.pct_rect] {
                    restore(&mut scene, &base, bw, r);
                }
            }
            // logo : le morceau flotte doucement (« le lien » qui se fait) ; au repos si mouvement reduit
            let bob = if m.calm { 0.0 } else { (m.t * 2.2).sin() };
            self.logo_piece(&mut scene, &g, (3.0 * bob, -4.0 * bob - 2.0));
            self.bar(&mut scene, m, &g, f);
            let pct = format!("{} %", (f * 100.0).floor() as u32);
            let pw = self.measure(2, 15.0 * g.s, 0.0, &pct);
            self.text(&mut scene, 2, 15.0 * g.s, 0.0, g.x0 + g.bw - pw, g.line_y, MUTED, g.a, &pct);
        }
        self.base = Some(base);
        self.scene = Some(scene);
        if m.cover.clamp(0.0, 1.0) >= 1.0 {
            return self.scene.as_ref().unwrap();
        }
        let scene = self.scene.as_ref().unwrap();
        let out = match self.out.as_mut() {
            Some(o) if o.width() == w && o.height() == h => o,
            _ => self.out.insert(Pixmap::new(w, h).expect("pixmap")),
        };
        apply_cover_into(scene, out, m.cover, scale);
        self.out.as_ref().unwrap()
    }

    /// Remplissage de la barre (+ tranche 3D) et reflet qui glisse.
    fn bar(&mut self, pm: &mut Pixmap, m: &Model, g: &Geo, f: f32) {
        let (x0, bw, bh, y, s, a) = (g.x0, g.bw, g.bh, g.bar_y, g.s, g.a);
        let (w, h) = (pm.width(), pm.height());
        let fw = (bw * f).max(bh);
        fill_rr(pm, x0, y + 3.0 * s, fw, bh - 1.0 * s, bh / 2.0, col(ACCENT_EDGE, a));
        fill_rr(pm, x0, y, fw, bh - 1.0 * s, bh / 2.0, col(ACCENT, a));
        if m.calm {
            return;
        }
        let Some(clip) = rrect(x0, y, fw, bh, bh / 2.0) else { return };
        // masque persistant : seule la zone de la barre est remise a zero puis remplie (equivalent a un masque neuf)
        let mut mask = match self.mask.take() {
            Some(k) if k.width() == w && k.height() == h => k,
            _ => match Mask::new(w, h) {
                Some(k) => k,
                None => return,
            },
        };
        let r = g.bar_rect;
        let wu = w as usize;
        for row in r.y0..r.y1 {
            mask.data_mut()[row * wu + r.x0..row * wu + r.x1].fill(0);
        }
        mask.fill_path(&clip, FillRule::Winding, true, Transform::identity());
        let sweep = ((m.t * 0.55) % 1.0) * (fw + 120.0 * s) - 60.0 * s;
        let mut pt = Paint::default();
        pt.set_color(Color::from_rgba8(255, 255, 255, (46.0 * a) as u8));
        pt.anti_alias = true;
        let mut pb = PathBuilder::new();
        let (sx, sw) = (x0 + sweep, 36.0 * s);
        pb.move_to(sx + 10.0 * s, y);
        pb.line_to(sx + sw + 10.0 * s, y);
        pb.line_to(sx + sw - 10.0 * s, y + bh);
        pb.line_to(sx - 10.0 * s, y + bh);
        pb.close();
        if let Some(path) = pb.finish() {
            pm.fill_path(&path, &pt, FillRule::Winding, Transform::identity(), Some(&mask));
        }
        self.mask = Some(mask);
    }

    /// Barre de titre de la fenetre hote (36 px comme `.titlebar` du launcher) : logo a gauche, titre centre, trois boutons.
    /// `hover` : 0 reduire, 1 agrandir, 2 fermer.
    pub fn render_titlebar(&mut self, title: &str, w: u32, h: u32, scale: f32, hover: Option<u8>, maximized: bool) -> Pixmap {
        let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
        pm.fill(col(BG, 1.0));
        let (wf, hf) = (w as f32, h as f32);
        let s = scale;
        let bw = 46.0 * s;
        for i in 0..3u8 {
            let x = wf - bw * (3 - i) as f32;
            if hover == Some(i) {
                let c = if i == 2 { [0xF0, 0x70, 0x6E] } else { SURFACE2 };
                fill_rr(&mut pm, x, 0.0, bw, hf - 1.0, 0.0, col(c, 1.0));
            }
            let ink = if hover == Some(2) && i == 2 { [0x2A, 0x08, 0x08] } else { TEXT };
            let (cx, cy) = (x + bw / 2.0, (hf - 1.0) / 2.0);
            let mut pb = PathBuilder::new();
            let u = 5.0 * s;
            match i {
                0 => {
                    pb.move_to(cx - u, cy + 0.5);
                    pb.line_to(cx + u, cy + 0.5);
                }
                1 if !maximized => {
                    pb.move_to(cx - u + 0.5, cy - u + 0.5);
                    pb.line_to(cx + u - 0.5, cy - u + 0.5);
                    pb.line_to(cx + u - 0.5, cy + u - 0.5);
                    pb.line_to(cx - u + 0.5, cy + u - 0.5);
                    pb.close();
                }
                1 => {
                    let d = 2.0 * s;
                    pb.move_to(cx - u + 0.5, cy - u + d + 0.5);
                    pb.line_to(cx + u - d - 0.5, cy - u + d + 0.5);
                    pb.line_to(cx + u - d - 0.5, cy + u - 0.5);
                    pb.line_to(cx - u + 0.5, cy + u - 0.5);
                    pb.close();
                    pb.move_to(cx - u + d + 0.5, cy - u + d - 0.5);
                    pb.line_to(cx - u + d + 0.5, cy - u + 0.5);
                    pb.line_to(cx + u - 0.5, cy - u + 0.5);
                    pb.line_to(cx + u - 0.5, cy + u - d - 0.5);
                    pb.line_to(cx + u - d - 0.5, cy + u - d - 0.5);
                }
                _ => {
                    pb.move_to(cx - u, cy - u);
                    pb.line_to(cx + u, cy + u);
                    pb.move_to(cx + u, cy - u);
                    pb.line_to(cx - u, cy + u);
                }
            }
            if let Some(p) = pb.finish() {
                let st = Stroke { width: s.max(1.0), ..Stroke::default() };
                pm.stroke_path(&p, &paint(col(ink, 1.0)), &st, Transform::identity(), None);
            }
        }
        // logo (variante small) a gauche
        let lw = 18.0 * s;
        let lh = lw * 248.0 / 232.0;
        self.logo(&mut pm, 12.0 * s, (hf - 1.0 - lh) / 2.0, lw, (0.0, 0.0), 1.0);
        // titre centre (Space Grotesk 12 px, interlettrage .04em, --muted)
        let text = self.fit(2, 12.0 * s, 0.5 * s, title, wf - 2.0 * (bw * 3.0 + 8.0 * s));
        let tw = self.measure(2, 12.0 * s, 0.5 * s, &text);
        let lh2 = self.font(2).as_scaled(PxScale::from(12.0 * s)).height();
        self.text(&mut pm, 2, 12.0 * s, 0.5 * s, (wf - tw) / 2.0, (hf - 1.0 - lh2) / 2.0, MUTED, 1.0, &text);
        // filet bas
        let mut p = Paint::default();
        p.set_color(col(LINE, 1.0));
        pm.fill_rect(tiny_skia::Rect::from_xywh(0.0, hf - 1.0, wf, 1.0).unwrap(), &p, Transform::identity(), None);
        pm
    }
}

/// Recopie le rectangle `r` de `src` dans `dst` (memes dimensions, `w` pixels de large).
fn restore(dst: &mut Pixmap, src: &Pixmap, w: usize, r: Rect4) {
    if r.x0 >= r.x1 {
        return;
    }
    let (d, s) = (dst.data_mut(), src.data());
    for y in r.y0..r.y1 {
        let (a, b) = ((y * w + r.x0) * 4, (y * w + r.x1) * 4);
        d[a..b].copy_from_slice(&s[a..b]);
    }
}

/// Transforme la scene opaque `scene` en deux panneaux qui se rejoignent au centre, ecrits dans `out` : `cover` 0..1.
/// Hors des panneaux : transparent (le jeu est visible dessous) ; une ombre douce et un liseré d'accent marquent le bord
/// des panneaux. Pixels premultiplies.
fn apply_cover_into(scene: &Pixmap, out: &mut Pixmap, cover: f32, scale: f32) {
    let c = cover.clamp(0.0, 1.0);
    let (w, h) = (scene.width() as usize, scene.height() as usize);
    let l = ((w as f32 / 2.0 * c).floor() as usize).min(w); // panneau gauche : colonnes [0, l)
    let r = w.saturating_sub(l); // panneau droit : colonnes [r, w)
    let strip = ((3.0 * scale) as usize).max(2);
    let shadow = (28.0 * scale) as usize;
    let strip_a = 1.0 - ((c - 0.9) / 0.1).clamp(0.0, 1.0); // le liseré s'efface quand les panneaux se touchent
    let sh_a = 0.38 * strip_a;
    let shadow_a: Vec<u8> = (0..shadow).map(|k| (sh_a * (1.0 - k as f32 / shadow as f32).powi(2) * 255.0) as u8).collect();
    let src = scene.data();
    let data = out.data_mut();
    for y in 0..h {
        let src_row = &src[y * w * 4..(y + 1) * w * 4];
        let row = &mut data[y * w * 4..(y + 1) * w * 4];
        if l < r {
            row[..l * 4].copy_from_slice(&src_row[..l * 4]);
            row[r * 4..].copy_from_slice(&src_row[r * 4..]);
            row[l * 4..r * 4].fill(0);
            for (k, &a) in shadow_a.iter().enumerate() {
                for x in [l + k, r - 1 - k] {
                    if x >= l && x < r {
                        row[x * 4 + 3] = a; // noir premultiplie
                    }
                }
            }
        } else {
            row.copy_from_slice(src_row);
        }
        for k in 0..strip.min(l) {
            for x in [l - 1 - k, r + k] {
                if x < w {
                    let i = x * 4;
                    for j in 0..3 {
                        let d = row[i + j] as f32;
                        row[i + j] = (d + (ACCENT[j] as f32 - d) * strip_a) as u8;
                    }
                }
            }
        }
    }
}

/// Pixels BGRA (ordre Windows) d'une image rendue.
pub fn to_bgra(pm: &Pixmap) -> Vec<u8> {
    let mut v = Vec::new();
    to_bgra_into(pm, &mut v);
    v
}

/// Comme `to_bgra`, dans un tampon reutilisable (redimensionne si besoin, sans allocation ensuite).
pub fn to_bgra_into(pm: &Pixmap, out: &mut Vec<u8>) {
    let src = pm.data();
    out.resize(src.len(), 0);
    bgra_into_slice(src, out);
}

/// Echange R et B de `src` (RGBA premultiplie) vers `dst` (meme longueur, multiple de 4) : par exemple directement dans
/// la memoire d'une DIB section.
pub fn bgra_into_slice(src: &[u8], dst: &mut [u8]) {
    assert_eq!(src.len(), dst.len());
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
        let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        let v = (v & 0xFF00_FF00) | ((v & 0xFF) << 16) | ((v >> 16) & 0xFF);
        *d = v.to_le_bytes();
    }
}
