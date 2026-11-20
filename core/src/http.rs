//! Client HTTP commun de la chaîne de préparation (versions, loaders, assets, Java, mappings LAN).
//!
//! Un client `reqwest::Client::new()` n'a aucun délai : une connexion qui ne répond plus (Wi-Fi qui décroche,
//! proxy, CDN bloqué) laisse le téléchargement, donc le lancement, en attente sans fin. Ici : un délai de connexion
//! et un délai de lecture **par morceau** (il se réarme à chaque octet reçu), mais volontairement pas de délai
//! total, pour ne pas couper un gros fichier (JRE, installeur Forge) sur une connexion lente.

use std::time::Duration;

use crate::types::Error;

/// Établissement de la connexion TCP + TLS.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Durée maximale sans recevoir le moindre octet (en-têtes ou corps).
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Constructeur avec les délais par défaut ; l'appelant peut y ajouter un user-agent, etc.
pub fn builder() -> reqwest::ClientBuilder {
    builder_with(CONNECT_TIMEOUT, READ_TIMEOUT)
}

/// Comme [`builder`] avec des délais choisis (tests : évite d'attendre 30 s).
pub fn builder_with(connect: Duration, read: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().connect_timeout(connect).read_timeout(read)
}

/// Client avec les délais par défaut. Panique seulement si la pile TLS ne s'initialise pas, comme `Client::new()`.
pub fn client() -> reqwest::Client {
    builder().build().expect("client HTTP (initialisation TLS)")
}

/// Convertit une erreur `reqwest` en `Error::Network` avec un message lisible ; l'URL n'y figure jamais.
pub fn net_error(e: reqwest::Error) -> Error {
    let e = e.without_url();
    Error::Network(if e.is_timeout() {
        "délai dépassé : le serveur ne répond plus (vérifie ta connexion)".to_string()
    } else if e.is_connect() {
        "connexion impossible (vérifie ta connexion)".to_string()
    } else {
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serveur local qui accepte la connexion puis ne répond jamais : « connexion bloquée » réelle, sans mock HTTP.
    async fn silent_server() -> (String, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        (url, h)
    }

    #[tokio::test]
    async fn connexion_bloquee_echoue_en_network_vite() {
        let (url, h) = silent_server().await;
        let c = builder_with(Duration::from_secs(5), Duration::from_millis(300)).build().unwrap();
        let t = Instant::now();
        let err = c.get(&url).send().await.map_err(net_error).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(3), "doit échouer vite : {:?}", t.elapsed());
        match err {
            Error::Network(m) => assert!(m.contains("délai dépassé"), "{m}"),
            e => panic!("{e:?}"),
        }
        h.abort();
    }

    #[tokio::test]
    async fn port_ferme_est_une_erreur_de_connexion() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let err = client().get(format!("http://127.0.0.1:{port}/")).send().await.map_err(net_error).unwrap_err();
        match err {
            Error::Network(m) => {
                assert!(m.contains("connexion impossible"), "{m}");
                assert!(!m.contains("127.0.0.1"), "l'URL ne doit pas fuiter : {m}");
            }
            e => panic!("{e:?}"),
        }
    }

    #[tokio::test]
    async fn corps_lent_mais_vivant_n_est_pas_coupe() {
        // Délai de lecture 400 ms ; le corps dure ~1 s au total mais un morceau arrive toutes les 100 ms.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\n").await.unwrap();
            for _ in 0..10 {
                s.write_all(b"x").await.unwrap();
                s.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let c = builder_with(Duration::from_secs(5), Duration::from_millis(400)).build().unwrap();
        let bytes = c.get(&url).send().await.unwrap().bytes().await.unwrap();
        assert_eq!(bytes.len(), 10);
        h.abort();
    }

    #[tokio::test]
    async fn corps_bloque_en_cours_de_route_echoue() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nabc").await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let c = builder_with(Duration::from_secs(5), Duration::from_millis(300)).build().unwrap();
        let t = Instant::now();
        let resp = c.get(&url).send().await.unwrap();
        let err = resp.bytes().await.map_err(net_error).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(3));
        assert!(matches!(err, Error::Network(m) if m.contains("délai dépassé")));
        h.abort();
    }
}
