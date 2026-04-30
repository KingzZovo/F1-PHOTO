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

use anyhow::{bail, Context, Result};
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
            env::var("F1P_BUNDLED_PG_DIR").unwrap_or_else(|_| "./bundled-pg/bin".into()),
        );
        let data_dir = PathBuf::from(
            env::var("F1P_BUNDLED_PG_DATA").unwrap_or_else(|_| "./bundled-pg-data".into()),
        );
        let port: u16 = env::var("F1P_BUNDLED_PG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let password =
            env::var("F1P_BUNDLED_PG_PASSWORD").unwrap_or_else(|_| "f1photo_prod".into());

        let initdb = which(&bin_dir, "initdb")?;
        let postgres = which(&bin_dir, "postgres")?;

        if !data_dir.join("PG_VERSION").exists() {
            tracing::info!(?data_dir, "initdb: creating bundled cluster");
            std::fs::create_dir_all(&data_dir).ok();
            let pwfile = data_dir.with_extension("pwfile");
            std::fs::write(&pwfile, &password).context("write initdb pwfile")?;
            let output = Command::new(&initdb)
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
                .output()
                .context("spawn initdb")?;
            let status = output.status;
            if !status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("initdb failed (exit={status}):\n{}", stderr.trim());
            }
            let _ = std::fs::remove_file(&pwfile);
        }

        // Make sure pgvector + listen-on-loopback are configured.
        write_postgresql_conf(&data_dir, port)?;
        write_pg_hba_conf(&data_dir)?;

        // Safety guard: handle the case where the target port is already in use.
        //
        // Default: fail-fast.
        // If we can prove the listener is our own previous bundled postgres
        // (via pidfile + /proc cmdline checks), SIGTERM it and retry once.
        let pidfile = data_dir.join("f1photo_bundled_pg.pid");
        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .expect("parse loopback addr");
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            if try_kill_previous_bundled_postgres(&pidfile, &data_dir, port, &postgres)? {
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline {
                    if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200))
                        .is_err()
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }

            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                bail!("bundled postgres port {port} is already in use (refusing to start). Stop the existing postgres or choose a different F1P_BUNDLED_PG_PORT.");
            }
        }

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

        // Record child pid so we can safely terminate it on future boots (if needed).
        let _ = std::fs::write(
            data_dir.join("f1photo_bundled_pg.pid"),
            child.id().to_string(),
        );

        wait_for_listen(port, Duration::from_secs(20))?;

        // initdb only creates the default `postgres` cluster DB; explicitly
        // create the application database (idempotent) before any sqlx
        // migrations try to connect.
        ensure_database(&bin_dir, port, &password, DEFAULT_USER, DEFAULT_DB)?;

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

fn try_kill_previous_bundled_postgres(
    pidfile: &Path,
    data_dir: &Path,
    port: u16,
    postgres_path: &Path,
) -> Result<bool> {
    let pid_str = match std::fs::read_to_string(pidfile) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let pid: u32 = pid_str.trim().parse().unwrap_or(0);
    if pid == 0 {
        return Ok(false);
    }

    // Validate cmdline matches our expected bundled postgres invocation.
    #[cfg(not(unix))]
    {
        // No reliable cross-platform way to verify ownership of the listener here.
        // Keep fail-fast behavior instead of risking killing the wrong process.
        let _ = pidfile;
        let _ = data_dir;
        let _ = port;
        let _ = postgres_path;
        return Ok(false);
    }

    #[cfg(unix)]
    let cmdline_path = PathBuf::from(format!("/proc/{pid}/cmdline"));
    if !cmdline_path.exists() {
        let _ = std::fs::remove_file(pidfile);
        return Ok(false);
    }
    let cmdline = std::fs::read(&cmdline_path).context("read existing postgres cmdline")?;
    let cmd = String::from_utf8_lossy(&cmdline).replace("\0", " ");

    let expected = format!(
        "{} -D {} -p {} -h 127.0.0.1",
        postgres_path.display(),
        data_dir.display(),
        port
    );
    if !cmd.contains(&expected) {
        return Ok(false);
    }

    tracing::warn!(
        pid,
        port,
        ?data_dir,
        "bundled postgres port is busy; terminating previous bundled postgres"
    );

    #[cfg(unix)]
    {
        unsafe {
            libc_kill(pid as i32, 15 /* SIGTERM */);
        }

        // Best-effort: if SIGTERM doesn"t stop it quickly, escalate to SIGKILL.
        for _ in 0..50 {
            if !PathBuf::from(format!("/proc/{pid}")).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100))
        }
        if PathBuf::from(format!("/proc/{pid}")).exists() {
            unsafe {
                libc_kill(pid as i32, 9 /* SIGKILL */);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    Ok(true)
}

fn write_postgresql_conf(data_dir: &Path, port: u16) -> Result<()> {
    let path = data_dir.join("postgresql.conf");
    // Keep the unix socket inside the data dir; the default `/var/run/postgresql`
    // requires root and breaks when the bundled PG runs as the f1photo system
    // user.
    let socket_dir = data_dir.display();
    let content = format!(
        "# managed by f1-photo bundled_pg.rs\n\
listen_addresses = '127.0.0.1'\n\
port = {port}\n\
unix_socket_directories = '{socket_dir}'\n\
shared_buffers = '256MB'\n\
work_mem = '32MB'\n\
max_connections = 100\n\
logging_collector = on\n\
log_directory = 'log'\n\
log_filename = 'postgresql-%Y-%m-%d.log'\n\
log_min_duration_statement = 500\n\
# pgvector NOTE: do NOT preload via shared_preload_libraries.\n\
# Migrations run `CREATE EXTENSION IF NOT EXISTS vector` and HNSW indexes\n\
# build fine without preloading. Preloading only matters for HNSW background\n\
# concurrent builds, and on bundled portable PG it triggers a SIGSEGV when\n\
# the .so isn't loaded from the same compile-time path as the binary.\n"
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

/// Idempotently `CREATE DATABASE` for the application using the bundled `psql`.
/// Connects to the default `postgres` cluster DB. Safe to call on every boot.
fn ensure_database(bin_dir: &Path, port: u16, password: &str, user: &str, db: &str) -> Result<()> {
    let psql = which(bin_dir, "psql")?;
    // Check if DB exists.
    let check = Command::new(&psql)
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(port.to_string())
        .arg("-U")
        .arg(user)
        .arg("-d")
        .arg("postgres")
        .arg("-tAc")
        .arg(format!("SELECT 1 FROM pg_database WHERE datname='{db}'"))
        .env("PGPASSWORD", password)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawn psql (check db)")?;
    if !check.status.success() {
        let stderr = String::from_utf8_lossy(&check.stderr);
        bail!("psql failed checking for {db}: {}", stderr.trim());
    }
    let exists = String::from_utf8_lossy(&check.stdout).trim() == "1";
    if exists {
        return Ok(());
    }
    tracing::info!(%db, "creating bundled application database");
    let create = Command::new(&psql)
        .arg("-h")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(port.to_string())
        .arg("-U")
        .arg(user)
        .arg("-d")
        .arg("postgres")
        .arg("-c")
        .arg(format!("CREATE DATABASE \"{db}\""))
        .env("PGPASSWORD", password)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawn psql (create db)")?;
    if !create.status.success() {
        let stderr = String::from_utf8_lossy(&create.stderr);
        bail!("psql failed creating {db}: {}", stderr.trim());
    }
    Ok(())
}
