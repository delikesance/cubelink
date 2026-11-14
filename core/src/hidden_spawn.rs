//! Windows : crée un processus dont la PREMIÈRE fenêtre restera cachée (`STARTUPINFO` : `STARTF_USESHOWWINDOW` +
//! `SW_HIDE`, consigne que GLFW applique à son premier affichage).
//!
//! `std` et `tokio` ne savent pas passer `wShowWindow` (`CommandExt::show_window` est instable) : ce module appelle
//! `CreateProcessW` lui-même. Il ne sert QUE pour l'essai « fenêtre jamais affichée par le jeu » du jeu intégré à la
//! fenêtre du launcher ; le lancement normal reste celui de `tokio` (voir `launch_run`). Même environnement, même
//! dossier de travail, mêmes arguments, pas de console (`CREATE_NO_WINDOW`), entrée standard vide, stdout et stderr
//! dans un seul tube rendu à l'appelant.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::Arc;

use tokio::sync::watch;
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE, PROCESS_INFORMATION,
    STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW,
};

const SW_HIDE: u16 = 0;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// Processus créé par [`spawn`]. L'abandonner avant sa fin l'arrête (comme `kill_on_drop` côté `tokio`).
pub struct Process {
    handle: Arc<OwnedHandle>,
    pid: u32,
    /// Code de sortie, publié par le fil d'attente.
    done: watch::Receiver<Option<u32>>,
}

impl Process {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Attend la fin du processus (peut être abandonnée et rappelée).
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let code = *self.done.wait_for(|c| c.is_some()).await.map_err(|_| io::Error::other("attente du processus interrompue"))?;
        Ok(ExitStatus::from_raw(code.unwrap_or(1)))
    }

    /// Handle du processus, valide tant que `self` vit.
    pub fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle() as HANDLE
    }

    /// Arrêt forcé, par le handle du processus.
    pub fn kill(&self) {
        // SAFETY : le handle appartient à `self.handle`, valide tant que `self` vit.
        unsafe { TerminateProcess(self.handle.as_raw_handle() as HANDLE, 1) };
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.done.borrow().is_none() {
            self.kill();
        }
    }
}

/// Ligne de commande pour `CreateProcessW` : programme entre guillemets, puis chaque argument selon la règle
/// documentée de `CommandLineToArgvW` (celle que relit aussi le runtime C de java) : guillemets autour d'un argument
/// vide ou contenant espace, tabulation ou guillemet ; `n` antislashs avant un guillemet deviennent `2n + 1`, `n`
/// antislashs en fin d'argument entre guillemets deviennent `2n`.
pub fn command_line(program: &OsStr, args: &[String]) -> io::Result<Vec<u16>> {
    let mut out = vec![QUOTE];
    out.extend(program.encode_wide());
    out.push(QUOTE);
    for arg in args {
        out.push(b' ' as u16);
        let arg: Vec<u16> = arg.encode_utf16().collect();
        if !arg.is_empty() && !arg.iter().any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == QUOTE) {
            out.extend_from_slice(&arg);
            continue;
        }
        out.push(QUOTE);
        let mut backslashes = 0;
        for &c in &arg {
            if c == BACKSLASH {
                backslashes += 1;
            } else {
                if c == QUOTE {
                    out.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
                }
                backslashes = 0;
            }
            out.push(c);
        }
        out.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        out.push(QUOTE);
    }
    if out.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "caractère nul dans la ligne de commande"));
    }
    out.push(0);
    Ok(out)
}

/// Rend `handle` héritable par les processus enfants.
fn inheritable(handle: OwnedHandle) -> io::Result<OwnedHandle> {
    // SAFETY : handle valide, possédé par l'appelant.
    if unsafe { SetHandleInformation(handle.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

/// Lance `program args` dans `cwd`. Rend le processus et le bout « lecture » du tube où arrivent stdout et stderr.
pub fn spawn(program: &Path, args: &[String], cwd: &Path) -> io::Result<(Process, File)> {
    let mut line = command_line(program.as_os_str(), args)?;
    let dir: Vec<u16> = cwd.as_os_str().encode_wide().chain(Some(0)).collect();
    let (reader, writer) = io::pipe()?;
    // Les bouts donnés à l'enfant ne sont héritables que le temps de l'appel : ils sont fermés (RAII) dès la sortie de
    // cette fonction, succès ou échec. Le bout « lecture » ne l'est jamais.
    let writer = inheritable(writer.into())?;
    let nul = inheritable(File::open("NUL")?.into())?;
    // SAFETY : structures C sans invariant, valides remplies de zéros.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE;
    si.hStdInput = nul.as_raw_handle() as HANDLE;
    si.hStdOutput = writer.as_raw_handle() as HANDLE;
    si.hStdError = writer.as_raw_handle() as HANDLE;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY : `line` et `dir` sont terminés par un nul et vivent pendant l'appel ; `si` et `pi` sont valides ; pas de
    // nom d'application (Windows cherche le programme comme pour une commande) ; environnement nul = celui du parent.
    let ok = unsafe {
        CreateProcessW(std::ptr::null(), line.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 1, CREATE_NO_WINDOW, std::ptr::null(), dir.as_ptr(), &si, &mut pi)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY : `CreateProcessW` a réussi, ces deux handles sont à nous ; celui du fil principal ne sert pas.
    let (handle, _thread) = unsafe { (OwnedHandle::from_raw_handle(pi.hProcess as _), OwnedHandle::from_raw_handle(pi.hThread as _)) };
    let (tx, done) = watch::channel(None);
    // Construit AVANT le fil d'attente : si celui-ci ne démarre pas, l'abandon de `process` arrête l'enfant.
    let process = Process { handle: Arc::new(handle), pid: pi.dwProcessId, done };
    let waited = process.handle.clone();
    std::thread::Builder::new().name("cubelink-jeu-fin".into()).spawn(move || {
        let mut code = 1;
        // SAFETY : `waited` garde le handle ouvert pendant toute l'attente.
        unsafe {
            WaitForSingleObject(waited.as_raw_handle() as HANDLE, INFINITE);
            GetExitCodeProcess(waited.as_raw_handle() as HANDLE, &mut code);
        }
        let _ = tx.send(Some(code));
    })?;
    Ok((process, File::from(OwnedHandle::from(reader))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::System::Threading::GetStartupInfoW;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    /// Relit une ligne de commande comme Windows le fait pour l'enfant.
    fn parse(line: &[u16]) -> Vec<String> {
        let mut n = 0;
        // SAFETY : `line` est terminée par un nul ; le tableau rendu est libéré par `LocalFree`.
        unsafe {
            let argv = CommandLineToArgvW(line.as_ptr(), &mut n);
            assert!(!argv.is_null());
            let out = (0..n as usize)
                .map(|i| {
                    let p = *argv.add(i);
                    let len = (0..).take_while(|&k| *p.add(k) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
                })
                .collect();
            LocalFree(argv.cast());
            out
        }
    }

    #[test]
    fn ligne_de_commande_relue_a_l_identique() {
        let program = r"C:\Program Files\Java 21\bin\javaw.exe";
        let args: Vec<String> = [
            "-Xmx4096M",
            r"-Djava.library.path=C:\Users\Jean Dupont\jeu\natives",
            r"C:\dossier avec espaces\",
            r"C:\sans_espace\",
            r#"dit "bonjour""#,
            r#"antislash avant guillemet \" et \\" fin\\"#,
            "",
            "tab\tulation",
            "--gameDir",
            r"C:\Users\Jean Dupont\AppData\Roaming\.cubelink\instances\Mon pack",
            "accentué é",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let line = command_line(OsStr::new(program), &args).unwrap();
        let back = parse(&line);
        assert_eq!(back[0], program);
        assert_eq!(&back[1..], &args[..]);
        assert!(command_line(OsStr::new("java"), &["a\0b".to_string()]).is_err());
    }

    /// Second filtre de test, qui ne désigne aucun test : il marque le processus enfant (l'environnement, lui, est commun).
    const SONDE: &str = "cubelink-sonde-enfant";

    /// N'agit que relancée comme enfant par le test suivant : dit ce que le processus a reçu comme consigne d'affichage.
    #[test]
    fn sonde_consigne_d_affichage() {
        if !std::env::args().any(|a| a == SONDE) {
            return;
        }
        // SAFETY : structure C valide remplie de zéros, remplie par l'appel.
        let si = unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            GetStartupInfoW(&mut si);
            si
        };
        println!("SONDE affichage={} impose={}", si.wShowWindow, si.dwFlags & STARTF_USESHOWWINDOW != 0);
        eprintln!("SONDE erreur");
        std::process::exit(7);
    }

    #[tokio::test]
    async fn enfant_cache_sortie_capturee_code_de_sortie() {
        let exe = std::env::current_exe().unwrap();
        let args: Vec<String> = ["hidden_spawn::tests::sonde_consigne_d_affichage", SONDE, "--exact", "--nocapture"].iter().map(|s| s.to_string()).collect();
        let dir = tempfile::tempdir().unwrap();
        let (mut p, mut out) = spawn(&exe, &args, dir.path()).unwrap();
        assert_ne!(p.pid(), 0);
        let text = tokio::task::spawn_blocking(move || {
            let mut s = String::new();
            out.read_to_string(&mut s).map(|_| s)
        })
        .await
        .unwrap()
        .unwrap();
        assert!(text.contains("SONDE affichage=0 impose=true"), "consigne SW_HIDE vue par l'enfant : {text}");
        assert!(text.contains("SONDE erreur"), "stderr capturé aussi : {text}");
        assert_eq!(p.wait().await.unwrap().code(), Some(7));
        assert_eq!(p.wait().await.unwrap().code(), Some(7), "attente rappelable");
    }

    #[tokio::test]
    async fn arret_force_et_programme_introuvable() {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, _out) = spawn(Path::new("ping"), &["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()], dir.path()).unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_millis(300), p.wait()).await.is_err(), "encore en vie");
        p.kill();
        let st = tokio::time::timeout(std::time::Duration::from_secs(10), p.wait()).await.expect("arrêté").unwrap();
        assert_eq!(st.code(), Some(1));
        let e = spawn(Path::new(r"C:\introuvable\java.exe"), &[], dir.path()).err().expect("échec");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
