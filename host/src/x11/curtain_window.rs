//! Fenetre du rideau : contenu (texte et avancement), rendu ARGB, minuteur.

use std::sync::atomic::Ordering;
use std::time::Duration;

use x11rb::connection::RequestConnection;
use x11rb::protocol::xproto::*;

use super::{dispatch, embedded, guard_tick, run_if, watch, Shared};
use crate::curtain::{self, Curtain, Frame};
use crate::flow::{Cadence, Effect, Event};

/// Cadence du minuteur : rapide pendant une animation, lente (filet de securite) sinon.
const TICK_FAST: Duration = Duration::from_millis(16);
const TICK_IDLE: Duration = Duration::from_millis(250);

/// Moteur de rendu (caches de texte et de fond) et tampon BGRA reutilise.
pub(super) struct Render {
    curtain: Curtain,
    buf: Vec<u8>,
}

impl Render {
    pub(super) fn new() -> Render {
        Render { curtain: Curtain::new(), buf: Vec::new() }
    }
}

/// Tout ce que montre le rideau maintenant (une seule prise du verrou).
fn frame(s: &Shared) -> Frame {
    let st = &mut *s.lock();
    Frame::capture(&st.machine, &mut st.closer, &mut st.tracker, &st.title, &st.caption, st.curtain_t0)
}

fn cadence(s: &Shared) -> Cadence {
    match s.tick.load(Ordering::SeqCst) {
        x if x == Cadence::Fast as u8 => Cadence::Fast,
        x if x == Cadence::Slow as u8 => Cadence::Slow,
        _ => Cadence::Off,
    }
}

/// Regle la cadence du minuteur du rideau et reveille son fil.
pub(super) fn set_tick(s: &Shared, c: Cadence) {
    if s.tick.swap(c as u8, Ordering::SeqCst) != c as u8 {
        super::dbg(match c {
            Cadence::Off => "minuteur : arrete",
            Cadence::Fast => "minuteur : rapide",
            Cadence::Slow => "minuteur : lent",
        });
        wake(s);
    }
}

fn wake(s: &Shared) {
    if let Some(t) = s.ticker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        t.unpark();
    }
}

/// Relance le minuteur (filet de securite, gardes) s'il etait arrete.
pub(super) fn request_sync(s: &Shared) {
    if cadence(s) == Cadence::Off {
        set_tick(s, Cadence::Slow);
    }
}

/// Fil du minuteur : un battement toutes les 16 ms pendant une animation, 250 ms jeu rattache, aucun reveil sinon.
pub(super) fn start_ticker(s: &std::sync::Arc<Shared>) -> Result<(), String> {
    let s2 = s.clone();
    let h = std::thread::Builder::new()
        .name("cubelink-host-tick".into())
        .spawn(move || loop {
            if s2.stop.load(Ordering::SeqCst) {
                break;
            }
            match cadence(&s2) {
                Cadence::Off => std::thread::park(),
                Cadence::Fast => std::thread::park_timeout(TICK_FAST),
                Cadence::Slow => std::thread::park_timeout(TICK_IDLE),
            }
            if !s2.stop.load(Ordering::SeqCst) && cadence(&s2) != Cadence::Off {
                tick(&s2);
            }
        })
        .map_err(|e| format!("fil du minuteur : {e}"))?;
    *s.ticker.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.thread().clone());
    Ok(())
}

/// Battement : fait avancer la machine, execute ses effets, redessine, applique les gardes.
fn tick(s: &Shared) {
    let todo = dispatch(&mut s.lock(), Event::Tick);
    // panneaux fermes : la machine bascule sans que le joueur voie la coupure (jeu <-> launcher)
    run_if(s, &todo, |e| matches!(e, Effect::PlaceGame | Effect::HideGame));
    embedded::settle_pose(s, false);
    embedded::sync(s);
    if todo.has(|e| *e == Effect::HideCurtain) {
        embedded::curtain_off(s);
    } else if s.curtain_mapped.load(Ordering::SeqCst) {
        present(s);
    }
    s.flush();
    // au repos : filet de securite lent (adoption tardive) ; en animation : 16 ms ; ni animation ni jeu : plus aucun reveil
    for e in &todo.effects {
        if let Effect::Timer(c) = *e {
            set_tick(s, c);
            if c == Cadence::Slow {
                watch::scan(s, false);
            }
        }
    }
    embedded::keep_curtain_on_top(s);
    // en dernier : un repli referme le rideau et remet lui-meme le minuteur en cadence rapide
    guard_tick(s);
}

/// Dessine le rideau (panneaux + contenu) avec transparence par pixel : rendu logiciel, envoye par bandes (`PutImage`).
pub(super) fn present(s: &Shared) {
    let Some((_, _, w, h)) = s.lock().curtain_rect else { return };
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let f = frame(s);
    let m = f.model();
    let mut r = s.render.lock().unwrap_or_else(|e| e.into_inner());
    let Render { curtain, buf } = &mut *r;
    let pm = curtain.render_ref(&m, w as u32, h as u32, s.scale);
    if pm.data().len() != w * h * 4 {
        return;
    }
    buf.resize(pm.data().len(), 0);
    curtain::bgra_into_slice(pm.data(), buf);
    let row = w * 4;
    let max = (s.conn.maximum_request_bytes().min(1 << 20)).saturating_sub(64);
    let rows = (max / row).clamp(1, h);
    let mut y = 0;
    while y < h {
        let n = rows.min(h - y);
        if s.conn.put_image(ImageFormat::Z_PIXMAP, s.curtain, s.gc, w as u16, n as u16, 0, y as i16, 0, s.depth, &buf[y * row..(y + n) * row]).is_err() {
            return;
        }
        y += n;
    }
    s.flush();
}
