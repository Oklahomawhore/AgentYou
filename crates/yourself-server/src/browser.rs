//! A separate Chromium host using CDP pipes and Chrome's native renderer sandbox.
use crate::data::{self, AppResult};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub fn chrome() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YOURSELF_CHROMIUM")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}
pub fn node() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YOURSELF_NODE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("node"))
        .find(|p| p.is_file())
}
pub fn available() -> bool {
    chrome().is_some() && node().is_some()
}
pub async fn execute(root: &Path, name: &str, args: &Value) -> AppResult<Value> {
    let chrome =
        chrome().ok_or("未找到 Chromium/Chrome；可用 YOURSELF_CHROMIUM 指定本机二进制。")?;
    let profiles = root.join(".browser");
    std::fs::create_dir_all(&profiles).map_err(|e| e.to_string())?;
    if !profiles
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(root)
    {
        return Err("浏览器配置目录越界".into());
    }
    let profile = profiles.join(data::id());
    let mut input = json!({"chrome":chrome,"profile":profile,"allow_private":cfg!(test)});
    match name {
        "Browser" => {
            input["url"] = args["url"].clone();
            if !input["url"].is_string() {
                return Err("Browser 缺少 url".into());
            }
        }
        "WebSearch" => {
            input["query"] = args["query"].clone();
            input["max_results"] = args.get("max_results").cloned().unwrap_or(json!(3));
        }
        _ => return Err("未知浏览器工具".into()),
    }
    let mut command =
        Command::new(node().ok_or("浏览器需要 Node.js；可用 YOURSELF_NODE 指定二进制")?);
    command
        .args([
            "--input-type=module",
            "-e",
            include_str!("../../../scripts/browser.mjs"),
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("浏览器需要 Node.js 22+：{e}"))?;
    struct Group(u32);
    impl Drop for Group {
        fn drop(&mut self) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
    let _group = Group(child.id().ok_or("浏览器没有进程 ID")?);
    let mut stdin = child.stdin.take().ok_or("浏览器输入不可用")?;
    let stdout = child.stdout.take().ok_or("浏览器输出不可用")?;
    let result = tokio::time::timeout(Duration::from_secs(70), async {
        stdin
            .write_all(input.to_string().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        drop(stdin);
        let mut bytes = vec![];
        stdout
            .take(1_000_001)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        child.wait().await.map_err(|e| e.to_string())?;
        if bytes.len() > 1_000_000 {
            return Err("浏览器结果超过上限".into());
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| "浏览器返回格式错误".into())
    })
    .await
    .map_err(|_| "浏览器超过 70 秒，已终止进程组".to_string())?;
    // Only our isolated, generated profile; never the user's Chrome profile.
    let _ = tokio::fs::remove_dir_all(&profile).await;
    result
}
