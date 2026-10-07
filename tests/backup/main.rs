// M5-006: drill backup/restore dengan PostgreSQL nyata. Skrip berjalan lewat `docker exec` ke container Postgres yang
// menerbitkan port 55432 (host tidak perlu klien PostgreSQL); test dilewati bila container itu tidak ada.
// Skema berasal dari migration asli (`sqlx::test`), dan hasil restore diperiksa dengan readiness M5-003.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use ai_team::{
    observability::{self, Component},
    store::artifact::ArtifactStore,
};
use sqlx::PgPool;
use uuid::Uuid;

struct Drill {
    container: String,
    source_db: String,
    root: PathBuf,
}

impl Drill {
    async fn new(pool: &PgPool) -> Option<Self> {
        let published = Command::new("docker")
            .args(["ps", "--filter", "publish=55432", "--format", "{{.Names}}"])
            .output()
            .ok()?;
        let container = String::from_utf8(published.stdout)
            .ok()?
            .lines()
            .next()?
            .to_owned();
        let source_db: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!("noctis-backup-drill-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("artifacts")).unwrap();
        Some(Self {
            container,
            source_db,
            root,
        })
    }

    fn script(&self, name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join(name)
    }

    /// Jalankan skrip dengan environment bersih (hanya yang dibutuhkan) supaya hasil tidak bergantung mesin.
    fn run(
        &self,
        script: &str,
        database: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> Output {
        let mut command = Command::new("bash");
        command
            .arg(self.script(script))
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("HOME", &self.root)
            .env("NOCTIS_PG_CONTAINER", &self.container)
            .env("NOCTIS_PG_USER", "postgres")
            .env("NOCTIS_PG_DATABASE", database);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn backup(&self, extra_env: &[(&str, &str)]) -> Output {
        let artifacts = self.root.join("artifacts");
        self.run(
            "backup.sh",
            &self.source_db,
            &[
                "--out",
                self.root.join("out").to_str().unwrap(),
                "--artifacts",
                artifacts.to_str().unwrap(),
            ],
            extra_env,
        )
    }

    fn archive(&self) -> Option<PathBuf> {
        let mut found: Vec<_> = fs::read_dir(self.root.join("out"))
            .ok()?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "gz"))
            .collect();
        found.sort();
        found.pop()
    }

    async fn new_empty_database(&self, pool: &PgPool) -> String {
        let name = format!("restore_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(pool)
            .await
            .unwrap();
        name
    }
}

impl Drop for Drill {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Fixture run: project, run, task, attempt, event, dan dua artifact (satu terdaftar di tabel, satu dirujuk event).
async fn seed(pool: &PgPool, drill: &Drill) -> (Uuid, String) {
    let (project, run, attempt) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'drill','/tmp/drill')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'drill','RUNNING',100000)")
        .bind(run).bind(project).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('dp','http://127.0.0.1:1','PRIMARY_API_KEY',1)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('dm','dp','m','coding',1000,1000)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('drill-task',$1,'worker','t','o','DONE','[\"a.txt\"]','[\"ok\"]','[\"true\"]',1000,1000,2)")
        .bind(run).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,finished_at,retain_until) VALUES ($1,'drill-task','worker','dp','dm',1,'completed','b',$2,now(),now(),now())")
        .bind(attempt).bind("0".repeat(40)).execute(pool).await.unwrap();
    let store = ArtifactStore::new(drill.root.join("artifacts"), 1024 * 1024).unwrap();
    let diff_id = Uuid::new_v4().to_string();
    let diff = store
        .write(
            &diff_id,
            "official.diff",
            "text/x-diff",
            b"diff --git a/a.txt b/a.txt\n",
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    sqlx::query("INSERT INTO artifacts (id,task_id,kind,logical_name,media_type,size_bytes,sha256) VALUES ($1::uuid,'drill-task','diff','official.diff','text/x-diff',$2,$3)")
        .bind(&diff_id).bind(i64::try_from(diff.size).unwrap()).bind(&diff.checksum).execute(pool).await.unwrap();
    store
        .write(
            "verify-drill-stdout",
            "stdout.txt",
            "text/plain",
            b"all good\n",
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,payload) VALUES ($1,'drill-task','system','verification','{\"artifact_ids\":[\"verify-drill-stdout\"]}')")
        .bind(run).execute(pool).await.unwrap();
    (run, diff_id)
}

async fn connect(pool: &PgPool, database: &str) -> PgPool {
    PgPool::connect_with(pool.connect_options().as_ref().clone().database(database))
        .await
        .unwrap()
}

macro_rules! drill {
    ($pool:expr) => {
        match Drill::new(&$pool).await {
            Some(drill) => drill,
            None => {
                eprintln!(
                    "skipped: container PostgreSQL yang menerbitkan port 55432 tidak ditemukan"
                );
                return;
            }
        }
    };
}

#[sqlx::test(migrations = "./migrations")]
async fn backup_then_restore_into_an_empty_environment_is_lossless(pool: PgPool) {
    let drill = drill!(pool);
    let (run, diff_id) = seed(&pool, &drill).await;

    let backup = drill.backup(&[]);
    assert_eq!(code(&backup), 0, "{}", stderr(&backup));
    let archive = drill.archive().expect("arsip harus ada");
    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o077,
        0,
        "arsip hanya boleh dibaca pemilik"
    );
    assert!(Path::new(&format!("{}.sha256", archive.display())).exists());
    let listing = String::from_utf8(
        Command::new("tar")
            .arg("-tzf")
            .arg(&archive)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    for member in [
        "db.sql",
        "info.json",
        "MANIFEST.sha256",
        &format!("artifacts/{diff_id}.artifact"),
        "artifacts/verify-drill-stdout.artifact",
    ] {
        assert!(
            listing
                .lines()
                .any(|line| line.trim_start_matches("./") == member),
            "{member} tidak ada di arsip:\n{listing}"
        );
    }

    let target = drill.new_empty_database(&pool).await;
    let restored_root = drill.root.join("restored-artifacts");
    let restore = drill.run(
        "restore.sh",
        &target,
        &[
            archive.to_str().unwrap(),
            "--artifacts",
            restored_root.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&restore), 0, "{}", stderr(&restore));

    // Isi database cocok, dan aplikasi menganggap hasil restore siap (migration valid).
    let restored = connect(&pool, &target).await;
    for table in [
        "tasks",
        "agent_runs",
        "artifacts",
        "events",
        "projects",
        "project_runs",
    ] {
        let query = format!("SELECT count(*) FROM {table}");
        let before: i64 = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
        let after: i64 = sqlx::query_scalar(&query)
            .fetch_one(&restored)
            .await
            .unwrap();
        assert_eq!(before, after, "{table}");
    }
    let readiness = observability::check(&restored, 60).await;
    assert_eq!(
        (readiness.database, readiness.migrations),
        (Component::Ok, Component::Ok)
    );
    let in_run: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE project_run_id=$1")
        .bind(run)
        .fetch_one(&restored)
        .await
        .unwrap();
    assert_eq!(in_run, 1);
    // Artifact identik dan terbaca lewat ArtifactStore (termasuk verifikasi checksum metadata-nya).
    let store = ArtifactStore::new(&restored_root, 1024 * 1024).unwrap();
    assert_eq!(
        store.read(&diff_id).unwrap(),
        b"diff --git a/a.txt b/a.txt\n"
    );
    assert_eq!(store.read("verify-drill-stdout").unwrap(), b"all good\n");
    assert_eq!(store.metadata(&diff_id).unwrap().checksum.len(), 64);
}

#[sqlx::test(migrations = "./migrations")]
async fn secrets_never_reach_the_archive(pool: PgPool) {
    let drill = drill!(pool);
    let (run, _) = seed(&pool, &drill).await;
    let secret = "unit-test-secret-value-98765";
    let plant = |payload: String| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,payload) VALUES ($1,'drill-task','system','note',$2::jsonb)")
                .bind(run).bind(payload).execute(&pool).await.unwrap();
        }
    };

    // 1) Nilai secret dari environment muncul di data.
    plant(format!("{{\"leak\":\"{secret}\"}}")).await;
    let output = drill.backup(&[("PRIMARY_API_KEY", secret)]);
    assert_eq!(code(&output), 3, "{}", stderr(&output));
    assert!(
        !stderr(&output).contains(secret),
        "pesan error tidak boleh membocorkan nilai secret"
    );
    assert!(
        drill.archive().is_none(),
        "tidak boleh ada arsip bila secret terdeteksi"
    );

    // 2) Bentuk secret umum (kunci privat PEM) di data, walau tidak ada di environment.
    sqlx::query("DELETE FROM events WHERE event_type='note'")
        .execute(&pool)
        .await
        .unwrap();
    plant("{\"k\":\"-----BEGIN PRIVATE KEY-----\\nabc\\n-----END PRIVATE KEY-----\"}".to_owned())
        .await;
    let output = drill.backup(&[]);
    assert_eq!(code(&output), 3, "{}", stderr(&output));
    assert!(drill.archive().is_none());

    // 3) Data bersih + secret di environment yang tidak ada di data => sukses.
    sqlx::query("DELETE FROM events WHERE event_type='note'")
        .execute(&pool)
        .await
        .unwrap();
    let output = drill.backup(&[("PRIMARY_API_KEY", secret)]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let archive = drill.archive().unwrap();
    let dump = Command::new("tar")
        .arg("-xzOf")
        .arg(&archive)
        .arg("./db.sql")
        .output()
        .unwrap()
        .stdout;
    assert!(!String::from_utf8_lossy(&dump).contains(secret));
}

#[sqlx::test(migrations = "./migrations")]
async fn corrupt_source_artifact_aborts_the_backup(pool: PgPool) {
    let drill = drill!(pool);
    let (_, diff_id) = seed(&pool, &drill).await;
    let path = drill
        .root
        .join("artifacts")
        .join(format!("{diff_id}.artifact"));
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&path, bytes).unwrap();
    let output = drill.backup(&[]);
    assert_eq!(code(&output), 4, "{}", stderr(&output));
    assert!(drill.archive().is_none());
}

fn repack(archive: &Path, mutate: impl FnOnce(&Path), name: &str) -> PathBuf {
    let dir = archive.parent().unwrap().join(format!("repack-{name}"));
    fs::create_dir_all(&dir).unwrap();
    assert!(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&dir)
            .status()
            .unwrap()
            .success()
    );
    mutate(&dir);
    let out = archive.parent().unwrap().join(format!("{name}.tar.gz"));
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(&dir)
            .arg(".")
            .status()
            .unwrap()
            .success()
    );
    out
}

#[sqlx::test(migrations = "./migrations")]
async fn restore_refuses_tampered_unsafe_or_non_empty_targets(pool: PgPool) {
    let drill = drill!(pool);
    let (_, diff_id) = seed(&pool, &drill).await;
    assert_eq!(code(&drill.backup(&[])), 0);
    let archive = drill.archive().unwrap();
    let artifacts = drill.root.join("restored");
    let restore = |archive: &Path, database: &str, root: &Path| {
        drill.run(
            "restore.sh",
            database,
            &[
                archive.to_str().unwrap(),
                "--artifacts",
                root.to_str().unwrap(),
            ],
            &[],
        )
    };

    // a) arsip diubah setelah dibuat: checksum arsip tidak cocok.
    let flipped = drill.root.join("out/flipped.tar.gz");
    let mut bytes = fs::read(&archive).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&flipped, bytes).unwrap();
    fs::copy(
        format!("{}.sha256", archive.display()),
        format!("{}.sha256", flipped.display()),
    )
    .unwrap();
    // nama di berkas .sha256 menunjuk arsip asli; salin isi dengan nama baru
    let sum = fs::read_to_string(format!("{}.sha256", archive.display())).unwrap();
    fs::write(
        format!("{}.sha256", flipped.display()),
        sum.replace(
            archive.file_name().unwrap().to_str().unwrap(),
            "flipped.tar.gz",
        ),
    )
    .unwrap();
    let target = drill.new_empty_database(&pool).await;
    assert_eq!(code(&restore(&flipped, &target, &artifacts)), 5);

    // b) isi diubah dan dikemas ulang tanpa .sha256: manifest di dalam arsip menangkapnya.
    let tampered = repack(
        &archive,
        |dir| {
            let path = dir.join(format!("artifacts/{diff_id}.artifact"));
            fs::write(path, b"evil").unwrap();
        },
        "tampered",
    );
    let output = restore(&tampered, &target, &artifacts);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(
        !artifacts.exists() || fs::read_dir(&artifacts).unwrap().next().is_none(),
        "tidak ada yang ditulis"
    );
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'",
    )
    .fetch_one(&connect(&pool, &target).await)
    .await
    .unwrap();
    assert_eq!(tables, 0, "database tujuan tetap kosong");

    // b2) dump SQL diubah (suntikan perintah) dan dikemas ulang: hanya manifest yang bisa menangkapnya.
    let injected = repack(
        &archive,
        |dir| {
            use std::io::Write;
            let mut sql = fs::OpenOptions::new()
                .append(true)
                .open(dir.join("db.sql"))
                .unwrap();
            writeln!(sql, "DROP TABLE tasks CASCADE;").unwrap();
        },
        "injected",
    );
    let output = restore(&injected, &target, &artifacts);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stderr(&output).contains("manifest"), "{}", stderr(&output));

    // c) anggota berupa symlink / path keluar direktori.
    let linked = repack(
        &archive,
        |dir| std::os::unix::fs::symlink("/etc/passwd", dir.join("evil-link")).unwrap(),
        "linked",
    );
    assert_eq!(code(&restore(&linked, &target, &artifacts)), 5);
    let traversal = drill.root.join("out/traversal.tar.gz");
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&traversal)
            .arg("--transform")
            .arg("s,^,../,")
            .arg("-C")
            .arg(&drill.root)
            .arg("artifacts")
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(code(&restore(&traversal, &target, &artifacts)), 5);

    // d) tujuan tidak kosong: database sumber (sudah berisi tabel) dan direktori artifact berisi.
    assert_eq!(code(&restore(&archive, &drill.source_db, &artifacts)), 6);
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("existing"), b"x").unwrap();
    assert_eq!(code(&restore(&archive, &target, &artifacts)), 6);
}
