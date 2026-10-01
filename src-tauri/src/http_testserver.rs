//! 极简本地 HTTP 文件服务器（仅用于自检，验证真实下载/断点续传链路）
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub struct TestServer {
    pub port: u16,
    pub root: PathBuf,
    pub requests: Arc<AtomicU64>,
    pub range_requests: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl TestServer {
    pub fn url(&self, file: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, file)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "txt" | "srt" | "vtt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// 启动一个只读静态文件服务，返回端口
pub fn serve_dir(root: &Path) -> std::io::Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(AtomicU64::new(0));
    let range_requests = Arc::new(AtomicU64::new(0));

    let root_buf = root.to_path_buf();
    let root_thread = root_buf.clone();
    let stop2 = stop.clone();
    let req2 = requests.clone();
    let rng2 = range_requests.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(s) = stream {
                let root = root_thread.clone();
                let req = req2.clone();
                let rng = rng2.clone();
                std::thread::spawn(move || {
                    let _ = handle(s, &root, &req, &rng);
                });
            }
        }
    });

    Ok(TestServer { port, root: root_buf, requests, range_requests, stop })
}

fn sanitize(root: &Path, raw: &str) -> Option<PathBuf> {
    let clean = raw.split('?').next().unwrap_or("").trim_start_matches('/');
    let decoded = clean.replace("%20", " ");
    let p = root.join(decoded);
    // 防目录穿越
    let canon_root = root.canonicalize().ok()?;
    let canon = p.canonicalize().ok()?;
    if canon.starts_with(&canon_root) && canon.is_file() {
        Some(canon)
    } else {
        None
    }
}

fn handle(
    mut s: TcpStream,
    root: &Path,
    requests: &Arc<AtomicU64>,
    range_requests: &Arc<AtomicU64>,
) -> std::io::Result<()> {
    requests.fetch_add(1, Ordering::SeqCst);
    let mut reader = BufReader::new(s.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut range: Option<(u64, Option<u64>)> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(v) = lower.strip_prefix("range:") {
            range_requests.fetch_add(1, Ordering::SeqCst);
            let v = v.trim();
            if let Some(spec) = v.strip_prefix("bytes=") {
                let spec = spec.split(',').next().unwrap_or("");
                let (a, b) = spec.split_once('-').unwrap_or(("", ""));
                let start = a.trim().parse::<u64>().unwrap_or(0);
                let end = b.trim().parse::<u64>().ok();
                range = Some((start, end));
            }
        }
    }

    let path = match sanitize(root, &target) {
        Some(p) => p,
        None => {
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return Ok(());
        }
    };
    let meta = std::fs::metadata(&path)?;
    let total = meta.len();
    let ct = content_type(&path);

    if method == "HEAD" {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
        );
        s.write_all(head.as_bytes())?;
        return Ok(());
    }

    if let Some((start, end)) = range {
        let end = end.unwrap_or(total.saturating_sub(1)).min(total.saturating_sub(1));
        if start > end || start >= total {
            let head = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            s.write_all(head.as_bytes())?;
            return Ok(());
        }
        let len = end - start + 1;
        let head = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nContent-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
        );
        s.write_all(head.as_bytes())?;
        let mut f = std::fs::File::open(&path)?;
        f.seek(SeekFrom::Start(start))?;
        let mut limited = f.take(len);
        std::io::copy(&mut limited, &mut s)?;
        return Ok(());
    }

    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
    );
    s.write_all(head.as_bytes())?;
    if method != "GET" {
        return Ok(());
    }
    let mut f = std::fs::File::open(&path)?;
    let mut buf = vec![0u8; 65536];
    let mut sent: u64 = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        s.write_all(&buf[..n])?;
        sent += n as u64;
        if sent >= total {
            break;
        }
    }
    let _ = s.flush();
    Ok(())
}
