use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::*;

pub struct Db {
    conn: Mutex<Connection>,
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS downloads (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '',
            thumbnail TEXT,
            duration REAL,
            uploader TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            progress REAL NOT NULL DEFAULT 0,
            speed TEXT,
            eta TEXT,
            downloaded INTEGER,
            total INTEGER,
            file_path TEXT,
            error TEXT,
            format_note TEXT,
            created_time INTEGER NOT NULL DEFAULT 0,
            request TEXT
        );
        CREATE TABLE IF NOT EXISTS converts (
            id TEXT PRIMARY KEY,
            input_file TEXT NOT NULL DEFAULT '',
            output_file TEXT NOT NULL DEFAULT '',
            format TEXT NOT NULL DEFAULT '',
            codec TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            progress REAL NOT NULL DEFAULT 0,
            error TEXT,
            created_time INTEGER NOT NULL DEFAULT 0,
            request TEXT
        );
        CREATE TABLE IF NOT EXISTS subtitles (
            id TEXT PRIMARY KEY,
            video_path TEXT NOT NULL DEFAULT '',
            language TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            subtitle_path TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            progress REAL NOT NULL DEFAULT 0,
            error TEXT,
            created_time INTEGER NOT NULL DEFAULT 0,
            request TEXT
        );
        CREATE TABLE IF NOT EXISTS kv (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_downloads_created ON downloads(created_time DESC);
        CREATE INDEX IF NOT EXISTS idx_converts_created ON converts(created_time DESC);
        CREATE INDEX IF NOT EXISTS idx_subtitles_created ON subtitles(created_time DESC);
        "#,
    )
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 内存库（测试用）
    pub fn in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /* ---------------- 下载 ---------------- */

    pub fn upsert_download(&self, t: &DownloadTask, request: Option<&str>) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"INSERT INTO downloads
               (id,title,url,thumbnail,duration,uploader,status,progress,speed,eta,downloaded,total,file_path,error,format_note,created_time,request)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
               ON CONFLICT(id) DO UPDATE SET
                 title=excluded.title, status=excluded.status, progress=excluded.progress,
                 speed=excluded.speed, eta=excluded.eta, downloaded=excluded.downloaded,
                 total=excluded.total, file_path=excluded.file_path, error=excluded.error,
                 format_note=excluded.format_note, thumbnail=excluded.thumbnail,
                 duration=excluded.duration, uploader=excluded.uploader,
                 request=COALESCE(excluded.request, downloads.request)"#,
            params![
                t.id,
                t.title,
                t.url,
                t.thumbnail,
                t.duration,
                t.uploader,
                t.status.as_str(),
                t.progress,
                t.speed,
                t.eta,
                t.downloaded,
                t.total,
                t.file_path,
                t.error,
                t.format_note,
                t.created_time,
                request,
            ],
        )?;
        Ok(())
    }

    pub fn list_downloads(&self) -> anyhow::Result<Vec<DownloadTask>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id,title,url,thumbnail,duration,uploader,status,progress,speed,eta,downloaded,total,file_path,error,format_note,created_time
             FROM downloads ORDER BY created_time DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DownloadTask {
                id: r.get(0)?,
                title: r.get(1)?,
                url: r.get(2)?,
                thumbnail: r.get(3)?,
                duration: r.get(4)?,
                uploader: r.get(5)?,
                status: TaskStatus::from_str(&r.get::<_, String>(6)?),
                progress: r.get(7)?,
                speed: r.get(8)?,
                eta: r.get(9)?,
                downloaded: r.get(10)?,
                total: r.get(11)?,
                file_path: r.get(12)?,
                error: r.get(13)?,
                format_note: r.get(14)?,
                created_time: r.get(15)?,
                file_exists: false,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_download(&self, id: &str) -> anyhow::Result<Option<DownloadTask>> {
        Ok(self.list_downloads()?.into_iter().find(|t| t.id == id))
    }

    pub fn get_download_request(&self, id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.lock();
        let v: Option<String> = conn
            .query_row("SELECT request FROM downloads WHERE id=?1", params![id], |r| r.get(0))
            .optional()?;
        Ok(v)
    }

    pub fn delete_download(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM downloads WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn clear_downloads(&self, done_only: bool) -> anyhow::Result<usize> {
        let conn = self.lock();
        let sql = if done_only {
            // handed_off（已交给 eMule 引擎）同样是「终态」，一并清掉
            "DELETE FROM downloads WHERE status IN ('done','error','canceled','handed_off')"
        } else {
            "DELETE FROM downloads"
        };
        Ok(conn.execute(sql, [])?)
    }

    /// 把上次异常退出时残留的进行中任务标记为暂停（可续传）。
    ///
    /// BUG-05：同时把「超出并发上限」的进行中任务降级成排队态（pending），
    /// 让它们等槽位释放后自动继续；已经在排队的任务保持排队（不再被压成暂停）。
    pub fn reset_stale_downloads(&self, concurrency: usize) -> anyhow::Result<usize> {
        let conn = self.lock();
        let limit = concurrency.max(1) as i64;
        let requeued = conn.execute(
            "UPDATE downloads SET status='pending', speed=NULL, eta=NULL
             WHERE status IN ('downloading','parsing')
               AND id NOT IN (
                 SELECT id FROM downloads WHERE status IN ('downloading','parsing')
                 ORDER BY created_time ASC LIMIT ?1
               )",
            params![limit],
        )?;
        let paused = conn.execute(
            "UPDATE downloads SET status='paused', speed=NULL, eta=NULL WHERE status IN ('downloading','parsing')",
            [],
        )?;
        Ok(paused + requeued)
    }

    /// 最早的排队任务（status='pending'）——队列泵据此唤醒下一个（BUG-05）
    pub fn next_queued_download(&self) -> anyhow::Result<Option<(String, Option<String>)>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id, request FROM downloads WHERE status='pending' ORDER BY created_time ASC LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?)
    }

    /// 测试/诊断用：按状态统计任务数
    pub fn count_downloads_by_status(&self, status: &str) -> anyhow::Result<i64> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE status=?1",
            params![status],
            |r| r.get(0),
        )?)
    }

    /* ---------------- 转换 ---------------- */

    pub fn upsert_convert(&self, t: &ConvertTask, request: Option<&str>) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"INSERT INTO converts (id,input_file,output_file,format,codec,status,progress,error,created_time,request)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
               ON CONFLICT(id) DO UPDATE SET
                 output_file=excluded.output_file, status=excluded.status, progress=excluded.progress,
                 error=excluded.error, codec=excluded.codec,
                 request=COALESCE(excluded.request, converts.request)"#,
            params![
                t.id,
                t.input_file,
                t.output_file,
                t.format,
                t.codec,
                t.status.as_str(),
                t.progress,
                t.error,
                t.created_time,
                request,
            ],
        )?;
        Ok(())
    }

    pub fn list_converts(&self) -> anyhow::Result<Vec<ConvertTask>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id,input_file,output_file,format,codec,status,progress,error,created_time
             FROM converts ORDER BY created_time DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConvertTask {
                id: r.get(0)?,
                input_file: r.get(1)?,
                output_file: r.get(2)?,
                format: r.get(3)?,
                codec: r.get(4)?,
                status: TaskStatus::from_str(&r.get::<_, String>(5)?),
                progress: r.get(6)?,
                error: r.get(7)?,
                created_time: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_convert(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM converts WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn get_convert(&self, id: &str) -> anyhow::Result<Option<ConvertTask>> {
        Ok(self.list_converts()?.into_iter().find(|t| t.id == id))
    }

    pub fn reset_stale_converts(&self) -> anyhow::Result<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "UPDATE converts SET status='error', error='应用上次退出时任务被中断' WHERE status IN ('converting','pending')",
            [],
        )?)
    }

    /* ---------------- 字幕 ---------------- */

    pub fn upsert_subtitle(&self, t: &SubtitleTask, request: Option<&str>) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute(
            r#"INSERT INTO subtitles (id,video_path,language,model,subtitle_path,status,progress,error,created_time,request)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
               ON CONFLICT(id) DO UPDATE SET
                 language=excluded.language, model=excluded.model, subtitle_path=excluded.subtitle_path,
                 status=excluded.status, progress=excluded.progress, error=excluded.error,
                 request=COALESCE(excluded.request, subtitles.request)"#,
            params![
                t.id,
                t.video_path,
                t.language,
                t.model,
                t.subtitle_path,
                t.status.as_str(),
                t.progress,
                t.error,
                t.created_time,
                request,
            ],
        )?;
        Ok(())
    }

    pub fn list_subtitles(&self) -> anyhow::Result<Vec<SubtitleTask>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id,video_path,language,model,subtitle_path,status,progress,error,created_time,request
             FROM subtitles ORDER BY created_time DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let request: Option<String> = r.get(9)?;
            Ok(SubtitleTask {
                id: r.get(0)?,
                video_path: r.get(1)?,
                language: r.get(2)?,
                model: r.get(3)?,
                subtitle_path: r.get(4)?,
                translate_to: request.as_deref().and_then(request_translate_to),
                status: TaskStatus::from_str(&r.get::<_, String>(5)?),
                progress: r.get(6)?,
                error: r.get(7)?,
                created_time: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_subtitle(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM subtitles WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn get_subtitle(&self, id: &str) -> anyhow::Result<Option<SubtitleTask>> {
        Ok(self.list_subtitles()?.into_iter().find(|t| t.id == id))
    }

    pub fn reset_stale_subtitles(&self) -> anyhow::Result<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "UPDATE subtitles SET status='error', error='应用上次退出时任务被中断' WHERE status IN ('transcribing','extracting','pending')",
            [],
        )?)
    }

    /* ---------------- KV ---------------- */

    pub fn kv_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.lock();
        Ok(conn
            .query_row("SELECT value FROM kv WHERE key=?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn count_all(&self) -> anyhow::Result<(i64, i64, i64)> {
        let conn = self.lock();
        let d: i64 = conn.query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))?;
        let c: i64 = conn.query_row("SELECT COUNT(*) FROM converts", [], |r| r.get(0))?;
        let s: i64 = conn.query_row("SELECT COUNT(*) FROM subtitles", [], |r| r.get(0))?;
        Ok((d, c, s))
    }
}

/// 从任务保存的 request JSON 里回填译文目标语言。
/// 队列行重启后也要能标出「→ 英文」（SubtitleTask 之前不回传 translate_to，只能靠前端内存里的
/// 一张表，重启即丢）。旧记录 / 字段缺失 / JSON 坏掉一律返回 None —— 只读一个字段，绝不因为
/// 一条脏记录让整个字幕列表打不开。
pub fn request_translate_to(request_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(request_json).ok()?;
    let t = v.get("translate_to")?.as_str()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

#[cfg(test)]
mod fix19_tests {
    use super::*;
    use crate::subtitle::new_subtitle_task;
    use crate::models::{SubtitleRequest, TaskStatus};

    fn mem_db() -> Db {
        Db::open(&std::env::temp_dir().join(format!("umi-db-19-{}.db", crate::ctx::short_id())))
            .unwrap()
    }

    /// 译文标记要能存进库、并在列表里回填（重启后队列行仍能标「→ 英文」）
    #[test]
    fn translate_flag_survives_roundtrip() {
        let db = mem_db();
        let mut req = SubtitleRequest {
            video_path: "C:/v/clip.mp4".into(),
            language: "auto".into(),
            model: Some("tiny".into()),
            translate_to: Some("en".into()),
            output_format: "srt".into(),
            output_dir: None,
        };
        let t = new_subtitle_task(&req, "tiny", 1);
        let json = serde_json::to_string(&req).unwrap();
        db.upsert_subtitle(&t, Some(&json)).unwrap();
        let got = db.get_subtitle(&t.id).unwrap().unwrap();
        assert_eq!(got.translate_to.as_deref(), Some("en"), "译文标记必须能读回来");
        assert_eq!(got.status, TaskStatus::Pending);

        // 不翻译的任务不能被误标
        req.translate_to = None;
        let t2 = new_subtitle_task(&req, "tiny", 2);
        db.upsert_subtitle(&t2, Some(&serde_json::to_string(&req).unwrap())).unwrap();
        assert_eq!(db.get_subtitle(&t2.id).unwrap().unwrap().translate_to, None);

        // 老记录（没有 request 列 / JSON 坏掉）→ None，绝不报错
        assert_eq!(request_translate_to(""), None);
        assert_eq!(request_translate_to("{"), None);
        assert_eq!(request_translate_to("{}"), None);
        assert_eq!(request_translate_to(r#"{"translate_to":null}"#), None);
        assert_eq!(request_translate_to(r#"{"translate_to":"  "}"#), None);
        assert_eq!(request_translate_to(r#"{"translate_to":"en"}"#), Some("en".into()));
    }
}
