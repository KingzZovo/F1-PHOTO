//! Optional portable Postgres bootstrap.
//!
//! When `F1P_USE_BUNDLED_PG=1` the server, on startup, will:
//!   1. `initdb` the `F1P_BUNDLED_PG_DATA` directory if it doesn't exist
//!      (using `F1P_BUNDLED_PG_DIR/bin/initdb`).
//!   2. Start a `postgres` child on `F1P_BUNDLED_PG_PORT` (default 5544),
//!      listening on 127.0.0.1.
//!   3. Wait for the socket to accept TCP, then create the `f1photo` role
//!      and `f1photo_prod` database (idempotent).
//!   4. Override `F1P_DATABASE_URL` (if not already explicitly set) so the
//!      rest of the boot process picks up the bundled instance.
//!
//! The child is parked on a watchdog task and SIGTERM'd on shutdown via
//! `BundledPg::shutdown`.
//!
//! Designed for the offline 10C/20T host where users cannot install PG
//! globally; the release tarball/zip ships a compiled portable PG tree
//! under `bundled-pg/` (Linux) or `bundled-pg\` (Windows).

use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 5544;
pub const DEFAULT_USER: &str = "f1photo";
pub const DEFAULT_DB: &str = "f1photo_prod";

pub struct BundledPg {
    pub port: u16,
    pub data_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub child: Option<Child>,
}

impl BundledPg {
    /// Starts the bundled PG if `F1P_USE_BUNDLED_PG=1`, returning `None`
    /// otherwise so callers can use whatever `F1P_DATABASE_URL` is set.
    pub fn maybe_start() -> Result<Option<Self>> {
        if env::var("F1P_USE_BUNDLED_PG").ok().as_deref() != Some("1") {
            return Ok(None);
        }

        let bin_dir = PathBuf::from(
            env::var("F1P_BUNDLED_PG_DIR")
                .unwrap_or_else(|_| "./bundled-pg/bin".into()),
        );
        let data_dir = PathBuf::from(
            env::var("F1P_BUNDLED_PG_DATA")
                .unwrap_or_else(|_| "./bundled-pg-data".into()),
        );
        let port: u16 = env::var("F1P_BUNDLED_PG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let password = env::var("F1P_BUNDLED_PG_PASSWORD")
            .unwrap_or_else(|_| "f1photo_prod".into());

        let initdb = which(&bin_dir, "initdb")?;
        let postgres = which(&bin_dir, "postgres")?;

        if !data_dir.join("PG_VERSION").exists() {
            tracing::info!(?data_dir, "initdb: creating bundled cluster");
            std::fs::create_dir_all(&data_dir).ok();
            let pwfile = data_dir.with_extension("pwfile");
            std::fs::write(&pwfile, &password).context("write initdb pwfile")?;
            let status = Command::new(&initdb)
                .arg("-D")
                .arg(&data_dir)
                .arg("-U")
                .arg(DEFAULT_USER)
                .arg("--auth=scram-sha-256")
                .arg("--encoding=UTF8")
                .arg("--locale=C")
                .arg(format!("--pwfile={}", pwfile.display()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .context("spawn initdb")?;
            let _ = std::fs::remove_file(&pwfile);
            if !status.success() {
                bail!("initdb failed (exit={status})");
            }
        }

        // Make sure pgvector + listen-on-loopback are configured.
        write_postgresql_conf(&data_dir, port)?;
        write_pg_hba_conf(&data_dir)?;

        tracing::info!(port, ?data_dir, "starting bundled postgres");
        let child = Command::new(&postgres)
            .arg("-D")
            .arg(&data_dir)
            .arg("-p")
            .arg(port.to_string())
            .arg("-h")
            .arg("127.0.0.1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn postgres")?;

        wait_for_listen(port, Duration::from_secs(20))?;

        // If the caller didn't explicitly set F1P_DATABASE_URL, point it at
        // the bundled instance so db::connect picks it up unchanged.
        if env::var_os("F1P_DATABASE_URL").is_none() {
            let url = format!(
                "postgres://{user}:{pw}@127.0.0.1:{port}/{db}",
                user = DEFAULT_USER,
                pw = password,
                port = port,
                db = DEFAULT_DB,
            );
            // SAFETY: only called once at boot before any threads observe env.
            unsafe { env::set_var("F1P_DATABASE_URL", &url) };
            tracing::info!("F1P_DATABASE_URL <- bundled");
        }

        Ok(Some(BundledPg {
            port,
            data_dir,
            bin_dir,
            child: Some(child),
        }))
    }

    /// Best-effort SIGTERM + wait for shutdown.
    pub fn shutdown(mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            {
                unsafe {
                    libc_kill(child.id() as i32, 15 /* SIGTERM */);
                }
                let _ = child.wait();
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
#[allow(non_snake_case)]
unsafe fn libc_kill(pid: i32, sig: i32) {
    let _ = unsafe { kill(pid, sig) };
}

fn which(bin_dir: &Path, name: &str) -> Result<PathBuf> {
    let exe = if cfg!(windows) {
        bin_dir.join(format!("{name}.exe"))
    } else {
        bin_dir.join(name)
    };
    if !exe.exists() {
        bail!(
            "bundled PG binary not found: {} (set F1P_BUNDLED_PG_DIR)",
            exe.display()
        );
    }
    Ok(exe)
}

fn write_postgresql_conf(data_dir: &Path, port: u16) -> Result<()> {
    let path = data_dir.join("postgresql.conf");
    let content = format!(
        "# managed by f1-photo bundled_pg.rs\nlisten_addresses = '127.0.0.1'\nport = {port}\nshared_buffers = '256MB'\nwork_mem = '32MB'\nmax_connections = 100\nlogging_collector = on\nlog_directory = 'log'\nlog_filename = 'postgresql-%Y-%m-%d.log'\nlog_min_duration_statement = 500\nshared_preload_libraries = 'vector'\n"
    );
    std::fs::write(&path, content).context("write postgresql.conf")?;
    Ok(())
}

fn write_pg_hba_conf(data_dir: &Path) -> Result<()> {
    let path = data_dir.join("pg_hba.conf");
    let content = "# managed by f1-photo bundled_pg.rs\nlocal   all             all                                     trust\nhost    all             all             127.0.0.1/32            scram-sha-256\nhost    all             all             ::1/128                 scram-sha-256\n";
    std::fs::write(&path, content).context("write pg_hba.conf")?;
    Ok(())
}

fn wait_for_listen(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let addr = format!("127.0.0.1:{port}");
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &addr.parse().expect("parse loopback addr"),
            Duration::from_millis(500),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("timed out waiting for bundled postgres on :{port}");
}
