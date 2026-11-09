//! Spike N1 : validation d'Iroh (tunnel QUIC <-> TCP echo, transfert par hash, reprise).
//! Usage : cargo run --release -- <mode> <test> [taille_Mo]
//!   mode : direct (relais desactive) | relay (relais local, IP desactive) | n0 (relais publics n0, Internet requis)
//!   test : tunnel | blobs | resume | all
use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use iroh::{
    endpoint::{presets, Connection},
    protocol::{AcceptError, ProtocolHandler, Router},
    tls::CaTlsConfig,
    Endpoint, EndpointAddr, RelayMap, RelayMode,
};
use iroh_blobs::{api::remote::GetProgressItem, store::fs::FsStore, BlobsProtocol};
use n0_future::StreamExt;
use rand::RngCore;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const TUNNEL_ALPN: &[u8] = b"cubelink/tunnel/0";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Direct,
    Relay,
    N0,
}

struct Ctx {
    mode: Mode,
    relay: Option<RelayMap>,
}

async fn make_endpoint(ctx: &Ctx, alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    let b = match ctx.mode {
        Mode::Direct => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
        Mode::Relay => Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(ctx.relay.clone().unwrap()))
            .ca_tls_config(CaTlsConfig::insecure_skip_verify())
            .clear_ip_transports(), // relais uniquement
        Mode::N0 if std::env::var("RELAY_ONLY").is_ok() => Endpoint::builder(presets::N0).clear_ip_transports(),
        Mode::N0 => Endpoint::builder(presets::N0),
    };
    Ok(b.alpns(alpns).bind().await?)
}

// ---------- tunnel ----------
#[derive(Debug, Clone)]
struct TunnelHandler {
    target: SocketAddr,
}

impl ProtocolHandler for TunnelHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let target = self.target;
            tokio::spawn(async move {
                let Ok(tcp) = TcpStream::connect(target).await else {
                    return;
                };
                let (mut r, mut w) = tcp.into_split();
                let a = tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut recv, &mut w).await;
                    let _ = w.shutdown().await;
                });
                let _ = tokio::io::copy(&mut r, &mut send).await;
                let _ = send.finish();
                let _ = a.await;
            });
        }
        Ok(())
    }
}

async fn echo_server() -> Result<SocketAddr> {
    let l = TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { break };
            tokio::spawn(async move {
                let (mut r, mut w) = s.into_split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    Ok(addr)
}

/// Cote B : ecoute TCP local, chaque connexion = un flux bidi QUIC vers A.
async fn tunnel_entry(conn: Connection) -> Result<SocketAddr> {
    let l = TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else { break };
            let conn = conn.clone();
            tokio::spawn(async move {
                let Ok((mut send, mut recv)) = conn.open_bi().await else {
                    return;
                };
                let (mut r, mut w) = tcp.into_split();
                let a = tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut r, &mut send).await;
                    let _ = send.finish();
                });
                let _ = tokio::io::copy(&mut recv, &mut w).await;
                let _ = a.await;
            });
        }
    });
    Ok(addr)
}

fn describe_paths(conn: &Connection) -> String {
    conn.paths()
        .iter()
        .map(|p| {
            format!(
                "{}{:?}{}",
                if p.is_relay() { "RELAY " } else { "IP " },
                p.remote_addr(),
                if p.is_selected() { " [selectionne]" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

async fn run_tunnel(ctx: &Ctx) -> Result<()> {
    println!("== TUNNEL ({:?}) ==", ctx.mode);
    let echo = echo_server().await?;
    let ep_a = make_endpoint(ctx, vec![TUNNEL_ALPN.to_vec()]).await?;
    let router_a = Router::builder(ep_a.clone())
        .accept(TUNNEL_ALPN, TunnelHandler { target: echo })
        .spawn();
    if ctx.mode != Mode::Direct {
        let _ = tokio::time::timeout(Duration::from_secs(15), ep_a.online()).await;
    }
    let addr_a: EndpointAddr = ep_a.addr();
    println!("A id={} addr={:?}", ep_a.id(), addr_a);

    let ep_b = make_endpoint(ctx, vec![]).await?;
    let t0 = Instant::now();
    let conn = ep_b.connect(addr_a, TUNNEL_ALPN).await?;
    println!("B connecte a A par cle publique en {:?}", t0.elapsed());
    let entry = tunnel_entry(conn.clone()).await?;

    // latence aller-retour : ping-pong 64 octets x 2000 via TCP->QUIC->TCP echo
    let mut s = TcpStream::connect(entry).await?;
    s.set_nodelay(true)?;
    let mut buf = [0u8; 64];
    for _ in 0..50 {
        s.write_all(&buf).await?;
        s.read_exact(&mut buf).await?;
    }
    let mut rtts = Vec::new();
    for i in 0..2000u32 {
        buf[..4].copy_from_slice(&i.to_le_bytes());
        let t = Instant::now();
        s.write_all(&buf).await?;
        s.read_exact(&mut buf).await?;
        rtts.push(t.elapsed().as_micros() as u64);
    }
    rtts.sort();
    let p = |q: f64| rtts[((rtts.len() as f64 * q) as usize).min(rtts.len() - 1)];
    println!(
        "RTT tunnel (64 o, n=2000) : min={}us p50={}us p95={}us p99={}us max={}us",
        rtts[0],
        p(0.5),
        p(0.95),
        p(0.99),
        rtts[rtts.len() - 1]
    );
    drop(s);

    // debit : 100 Mo aller + retour simultanes
    let total: usize = 100 * 1024 * 1024;
    let s = TcpStream::connect(entry).await?;
    let (mut r, mut w) = s.into_split();
    let t = Instant::now();
    let writer = tokio::spawn(async move {
        let chunk = vec![0xABu8; 64 * 1024];
        let mut sent = 0;
        while sent < total {
            w.write_all(&chunk).await?;
            sent += chunk.len();
        }
        w.shutdown().await?;
        anyhow::Ok(())
    });
    let mut got = 0usize;
    let mut b = vec![0u8; 256 * 1024];
    loop {
        let n = r.read(&mut b).await?;
        if n == 0 {
            break;
        }
        got += n;
    }
    writer.await??;
    let el = t.elapsed().as_secs_f64();
    println!(
        "Debit tunnel : {} Mo envoyes + {} Mo recus (echo) en {:.2}s => {:.1} Mo/s par sens ({:.0} Mbit/s)",
        total >> 20,
        got >> 20,
        el,
        (total as f64 / 1048576.0) / el,
        (total as f64 * 8.0 / 1e6) / el
    );
    println!("Chemins : {}", describe_paths(&conn));
    println!(
        "RTT QUIC rapporte : {:?}",
        conn.paths().iter().find(|p| p.is_selected()).map(|p| p.rtt())
    );
    conn.close(0u32.into(), b"done");
    ep_b.close().await;
    router_a.shutdown().await?;
    Ok(())
}

// ---------- blobs ----------
fn make_file(path: &PathBuf, mb: usize) -> Result<()> {
    let mut f = std::fs::File::create(path)?;
    let mut rng = rand::rng();
    let mut b = vec![0u8; 1 << 20];
    for _ in 0..mb {
        rng.fill_bytes(&mut b);
        std::io::Write::write_all(&mut f, &b)?;
    }
    Ok(())
}

async fn setup_blob_pair(
    ctx: &Ctx,
    dir: &PathBuf,
    file: &PathBuf,
) -> Result<(Router, FsStore, iroh_blobs::HashAndFormat, EndpointAddr)> {
    let store_a = FsStore::load(dir.join("store-a")).await?;
    let t = Instant::now();
    let tag = store_a.blobs().add_path(file.clone()).await?;
    println!(
        "A : import + hash BLAKE3 de {} Mo en {:?} -> {}",
        std::fs::metadata(file)?.len() >> 20,
        t.elapsed(),
        tag.hash
    );
    let ep_a = make_endpoint(ctx, vec![iroh_blobs::ALPN.to_vec()]).await?;
    let blobs = BlobsProtocol::new(&store_a, None);
    let router = Router::builder(ep_a.clone())
        .accept(iroh_blobs::ALPN, blobs)
        .spawn();
    if ctx.mode != Mode::Direct {
        let _ = tokio::time::timeout(Duration::from_secs(15), ep_a.online()).await;
    }
    Ok((router, store_a, tag.into(), ep_a.addr()))
}

async fn run_blobs(ctx: &Ctx, mb: usize) -> Result<()> {
    println!("== BLOBS ({:?}, {} Mo) ==", ctx.mode, mb);
    let dir = std::env::temp_dir().join(format!("cubelink-n1-blobs-{:?}", ctx.mode));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("source.bin");
    make_file(&file, mb)?;
    let (router_a, _sa, content, addr_a) = setup_blob_pair(ctx, &dir, &file).await?;
    let ep_b = make_endpoint(ctx, vec![]).await?;
    let store_b = FsStore::load(dir.join("store-b")).await?;
    let conn = ep_b.connect(addr_a, iroh_blobs::ALPN).await?;
    let t = Instant::now();
    let mut stream = store_b.remote().fetch(conn.clone(), content).stream();
    let mut stats = None;
    while let Some(item) = stream.next().await {
        match item {
            GetProgressItem::Done(s) => {
                stats = Some(s);
                break;
            }
            GetProgressItem::Error(e) => return Err(anyhow!("fetch error: {e:?}")),
            _ => {}
        }
    }
    let el = t.elapsed().as_secs_f64();
    let stats = stats.ok_or(anyhow!("pas de stats"))?;
    println!(
        "B : telecharge en {:.2}s => {:.1} Mo/s ({:.0} Mbit/s) ; payload lu={} ; total lu (avec arbre bao)={}",
        el,
        mb as f64 / el,
        mb as f64 * 8.0 / el,
        stats.payload_bytes_read,
        stats.total_bytes_read()
    );
    println!("Chemins : {}", describe_paths(&conn));
    let out = dir.join("out.bin");
    store_b.blobs().export(content.hash, out.clone()).await?;
    let (a, b) = (std::fs::read(&file)?, std::fs::read(&out)?);
    let h = iroh_blobs::Hash::new(&b);
    println!(
        "Verification : hash recalcule {} == attendu {} : {} ; octets identiques a la source : {}",
        h,
        content.hash,
        h == content.hash,
        a == b
    );
    anyhow::ensure!(a == b && h == content.hash, "verification KO");
    let bogus = iroh_blobs::Hash::new(b"inexistant");
    let r = store_b
        .remote()
        .fetch(conn.clone(), iroh_blobs::HashAndFormat::raw(bogus))
        .complete()
        .await;
    println!(
        "Hash inconnu cote A -> {}",
        match r {
            Ok(_) => "OK (inattendu)".to_string(),
            Err(e) => format!("erreur attendue: {e}"),
        }
    );
    conn.close(0u32.into(), b"done");
    ep_b.close().await;
    router_a.shutdown().await?;
    Ok(())
}

async fn run_resume(ctx: &Ctx, mb: usize) -> Result<()> {
    println!("== RESUME ({:?}, {} Mo) ==", ctx.mode, mb);
    let dir = std::env::temp_dir().join(format!("cubelink-n1-resume-{:?}", ctx.mode));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("source.bin");
    make_file(&file, mb)?;
    let (router_a, _sa, content, addr_a) = setup_blob_pair(ctx, &dir, &file).await?;
    let total = (mb as u64) << 20;
    let ep_b = make_endpoint(ctx, vec![]).await?;
    let store_b = FsStore::load(dir.join("store-b")).await?;
    let conn = ep_b.connect(addr_a.clone(), iroh_blobs::ALPN).await?;
    let mut stream = store_b.remote().fetch(conn.clone(), content).stream();
    let mut cut_at = 0u64;
    let mut failed = false;
    while let Some(item) = stream.next().await {
        match item {
            GetProgressItem::Progress(n) if n >= total * 4 / 10 && cut_at == 0 => {
                cut_at = n;
                conn.close(1u32.into(), b"coupure simulee");
            }
            GetProgressItem::Error(e) => {
                println!(
                    "1re tentative interrompue comme prevu (coupure vers {} Mo) : {e}",
                    cut_at >> 20
                );
                failed = true;
                break;
            }
            GetProgressItem::Done(_) => break,
            _ => {}
        }
    }
    anyhow::ensure!(failed, "la coupure n'a pas interrompu le transfert");
    drop(stream);
    ep_b.close().await;
    // simule un redemarrage du processus : fermeture puis reouverture du store sur disque
    store_b.shutdown().await?;
    drop(store_b);
    let store_b = FsStore::load(dir.join("store-b")).await?;
    let local = store_b.remote().local(content).await?;
    println!(
        "Apres redemarrage : {} Mo deja presents localement sur {} Mo (complet={})",
        local.local_bytes() >> 20,
        mb,
        local.is_complete()
    );
    let ep_b2 = make_endpoint(ctx, vec![]).await?;
    let conn2 = ep_b2.connect(addr_a, iroh_blobs::ALPN).await?;
    let t = Instant::now();
    let stats = store_b
        .remote()
        .fetch(conn2.clone(), content)
        .complete()
        .await
        .map_err(|e| anyhow!("{e:?}"))?;
    println!(
        "2e tentative : {:.2}s, payload lu={} Mo (reprise attendue ~{} Mo ; {} Mo si repart de zero)",
        t.elapsed().as_secs_f64(),
        stats.payload_bytes_read >> 20,
        mb as u64 - (local.local_bytes() >> 20),
        mb
    );
    let out = dir.join("out.bin");
    store_b.blobs().export(content.hash, out.clone()).await?;
    let h = iroh_blobs::Hash::new(&std::fs::read(&out)?);
    println!("Verification finale : hash identique = {}", h == content.hash);
    anyhow::ensure!(h == content.hash, "verification KO");
    conn2.close(0u32.into(), b"done");
    ep_b2.close().await;
    router_a.shutdown().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.get(1).map(String::as_str) {
        Some("relay") => Mode::Relay,
        Some("n0") => Mode::N0,
        _ => Mode::Direct,
    };
    let test = args.get(2).map(String::as_str).unwrap_or("all");
    let mb: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
    let (relay, _guard) = if mode == Mode::Relay {
        let (map, url, server) = iroh::test_utils::run_relay_server().await?;
        println!("Relais local de test : {url}");
        (Some(map), Some(server))
    } else {
        (None, None)
    };
    let ctx = Ctx { mode, relay };
    if matches!(test, "tunnel" | "all") {
        run_tunnel(&ctx).await?;
    }
    if matches!(test, "blobs" | "all") {
        run_blobs(&ctx, mb).await?;
    }
    if matches!(test, "resume" | "all") {
        run_resume(&ctx, mb).await?;
    }
    Ok(())
}
