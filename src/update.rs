//! 自動更新：查詢 GitHub Releases 最新版本，下載 zip、解壓到暫存資料夾，
//! 由外部腳本在程式關閉後覆蓋檔案並重新啟動。
//! 設定：data/update.json → {"repo": "擁有者/儲存庫", "asset": "lijing-windows.zip"}
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available { tag: String, url: String, notes: String, size: u64 },
    Downloading { got: u64, total: u64 },
    Ready,
    Failed(String),
    NotConfigured,
}

pub struct Updater {
    pub state: Arc<Mutex<State>>,
    repo: String,
    asset: String,
    app_dir: PathBuf,
}

fn parse_ver(s: &str) -> Vec<u32> {
    s.trim_start_matches(['v', 'V']).split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).map(|x| x.parse().unwrap_or(0)).collect()
}
pub fn newer(remote: &str, local: &str) -> bool {
    parse_ver(remote) > parse_ver(local)
}

impl Updater {
    pub fn new(data_dir: &Path) -> Updater {
        let cfg: serde_json::Value = std::fs::read(data_dir.join("update.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let repo = cfg["repo"].as_str().unwrap_or("").to_string();
        let default_asset = if cfg!(windows) { "lijing-windows.zip" } else { "lijing-linux.zip" };
        let asset = cfg["asset"].as_str().unwrap_or(default_asset).to_string();
        let app_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default();
        let st = if repo.is_empty() || repo.contains("OWNER") { State::NotConfigured } else { State::Idle };
        Updater { state: Arc::new(Mutex::new(st)), repo, asset, app_dir }
    }
    pub fn get(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    pub fn check(&self) {
        if self.repo.is_empty() || self.repo.contains("OWNER") { return; }
        *self.state.lock().unwrap() = State::Checking;
        let st = self.state.clone();
        let (repo, asset) = (self.repo.clone(), self.asset.clone());
        std::thread::spawn(move || {
            let r = (|| -> Result<State, String> {
                let url = format!("https://api.github.com/repos/{repo}/releases/latest");
                let mut resp = ureq::get(&url).header("User-Agent", "lijing-atlas-updater").header("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
                let j: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
                let tag = j["tag_name"].as_str().unwrap_or("").to_string();
                if !newer(&tag, VERSION) { return Ok(State::UpToDate); }
                let a = j["assets"].as_array().and_then(|v| v.iter().find(|a| a["name"].as_str() == Some(&asset)).cloned()).ok_or(format!("新版本 {tag} 沒有 {asset}"))?;
                Ok(State::Available { tag, url: a["browser_download_url"].as_str().unwrap_or("").to_string(), notes: j["body"].as_str().unwrap_or("").to_string(), size: a["size"].as_u64().unwrap_or(0) })
            })();
            *st.lock().unwrap() = r.unwrap_or_else(State::Failed);
        });
    }
    /// 下載並準備更新；完成後狀態為 Ready，呼叫 `restart()` 套用。
    pub fn download(&self) {
        let State::Available { url, size, .. } = self.get() else { return };
        *self.state.lock().unwrap() = State::Downloading { got: 0, total: size };
        let st = self.state.clone();
        let app_dir = self.app_dir.clone();
        std::thread::spawn(move || {
            let r = (|| -> Result<(), String> {
                let resp = ureq::get(&url).header("User-Agent", "lijing-atlas-updater").call().map_err(|e| e.to_string())?;
                let mut rd = resp.into_body().into_reader();
                let mut buf = Vec::with_capacity(size as usize);
                let mut chunk = vec![0u8; 1 << 16];
                loop {
                    let n = rd.read(&mut chunk).map_err(|e| e.to_string())?;
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                    *st.lock().unwrap() = State::Downloading { got: buf.len() as u64, total: size };
                }
                let stage = app_dir.join("_update");
                let _ = std::fs::remove_dir_all(&stage);
                std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
                let mut z = zip::ZipArchive::new(std::io::Cursor::new(buf)).map_err(|e| e.to_string())?;
                // 壓縮檔若有單一頂層資料夾則略過它
                let names: Vec<String> = (0..z.len()).filter_map(|i| z.by_index(i).ok().and_then(|f| f.name().ok().map(|n| n.to_string()))).collect();
                let prefix = names.first().and_then(|n| n.split('/').next()).filter(|p| names.iter().all(|n| n.starts_with(&format!("{p}/")))).map(|p| format!("{p}/")).unwrap_or_default();
                for i in 0..z.len() {
                    let mut f = z.by_index(i).map_err(|e| e.to_string())?;
                    let name = f.name().map(|n| n.trim_start_matches(&prefix).to_string()).unwrap_or_default();
                    if name.is_empty() || name.contains("..") { continue; }
                    let out = stage.join(&name);
                    if f.is_dir() { std::fs::create_dir_all(&out).ok(); continue; }
                    if let Some(p) = out.parent() { std::fs::create_dir_all(p).ok(); }
                    let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
                }
                Ok(())
            })();
            *st.lock().unwrap() = match r { Ok(_) => State::Ready, Err(e) => State::Failed(e) };
        });
    }
    /// 寫出套用腳本並結束程式：腳本等本程式結束 → 覆蓋檔案（保留使用者設定）→ 重新啟動。
    pub fn restart(&self) {
        let pid = std::process::id();
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = &self.app_dir;
        #[cfg(windows)]
        {
            let script = dir.join("_update.cmd");
            let body = format!(
                "@echo off\r\nchcp 65001 >nul\r\n:wait\r\ntasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul && (timeout /t 1 >nul & goto wait)\r\nrobocopy \"{d}\\_update\" \"{d}\" /E /MOVE /NFL /NDL /NJH /NJS >nul\r\nstart \"\" \"{exe}\"\r\ndel \"%~f0\"\r\n",
                d = dir.display(), exe = exe.display()
            );
            if std::fs::write(&script, body).is_ok() {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("cmd").args(["/C", &script.display().to_string()]).creation_flags(0x08000000).spawn();
            }
        }
        #[cfg(not(windows))]
        {
            let script = dir.join("_update.sh");
            let body = format!("#!/bin/sh\nwhile kill -0 {pid} 2>/dev/null; do sleep 1; done\ncp -r \"{d}/_update/.\" \"{d}/\" && rm -rf \"{d}/_update\"\nchmod +x \"{exe}\"\n\"{exe}\" &\nrm -- \"$0\"\n", d = dir.display(), exe = exe.display());
            if std::fs::write(&script, body).is_ok() {
                let _ = std::process::Command::new("sh").arg(&script).spawn();
            }
        }
        std::process::exit(0);
    }
}
