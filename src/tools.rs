//! Outside programs OmniSearch calls: where each one is, its version, and one
//! bounded run with the output kept. Nothing is pinned: whatever is installed
//! runs, and every caller checks the fields and exit codes it relies on
//! ([`contract_warning`]), so a changed tool fails loudly and falls through.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Donsetch,
    Moli,
    Obscura,
    Crwl,
    YtDlp,
    Gh,
}

impl Tool {
    pub const ALL: [Tool; 6] = [Tool::Donsetch, Tool::Moli, Tool::Obscura, Tool::Crwl, Tool::YtDlp, Tool::Gh];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Donsetch => "donsetch",
            Tool::Moli => "moli",
            Tool::Obscura => "obscura",
            Tool::Crwl => "crwl",
            Tool::YtDlp => "yt-dlp",
            Tool::Gh => "gh",
        }
    }

    fn exe(self) -> String {
        if cfg!(windows) {
            format!("{}.exe", self.name())
        } else {
            self.name().to_string()
        }
    }

    /// The config key in `[tools]` that overrides the path.
    fn configured(self) -> Option<String> {
        let t = &crate::config::get().tools;
        let v = match self {
            Tool::Donsetch => &t.donsetch,
            Tool::Moli => &t.moli,
            Tool::Obscura => &t.obscura,
            Tool::Crwl => &t.crwl,
            Tool::YtDlp => &t.ytdlp,
            Tool::Gh => &t.gh,
        };
        (!v.is_empty()).then(|| v.clone())
    }
}

/// OmniSearch's own tools folder: donsetch and Obscura live here, installed
/// and updated by `osearch update`.
pub fn tools_dir() -> PathBuf {
    crate::config::data_dir().join("tools")
}

/// Where a tool is: the config path, else OmniSearch's tools folder, else
/// PATH. `None` when it is not installed.
pub fn locate(tool: Tool) -> Option<PathBuf> {
    if let Some(p) = tool.configured() {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let own = tools_dir().join(tool.name()).join(tool.exe());
    if own.is_file() {
        return Some(own);
    }
    on_path(&tool.exe())
}

pub(crate) fn on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// One finished run of an outside tool.
#[derive(Debug, Clone)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub elapsed: Duration,
    pub timed_out: bool,
}

impl Ran {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Run `tool args` with a hard time limit; the process is killed on expiry.
/// The call goes into the run's call log and its stdout into `raw/`.
pub async fn run(tool: Tool, args: &[String], timeout: Duration) -> Result<Ran, String> {
    run_env(tool, args, timeout, &[]).await
}

/// `run` with extra environment variables for the tool.
pub async fn run_env(tool: Tool, args: &[String], timeout: Duration, env: &[(&str, &str)]) -> Result<Ran, String> {
    let path = locate(tool).ok_or_else(|| format!("{} is not installed", tool.name()))?;
    run_path(tool.name(), &path, args, timeout, env).await
}

async fn run_path(
    label: &str,
    path: &Path,
    args: &[String],
    timeout: Duration,
    env: &[(&str, &str)],
) -> Result<Ran, String> {
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new(path);
    cmd.envs(env.iter().copied());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Python tools print UTF-8 only when told to on Windows.
    cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{label}: cannot start {}: {e}", path.display()))?;
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let read_out = async {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf).await;
        buf
    };
    let read_err = async {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf).await;
        buf
    };
    let waited = tokio::time::timeout(timeout, async {
        let (o, e, status) = tokio::join!(read_out, read_err, child.wait());
        (o, e, status)
    })
    .await;
    let ran = match waited {
        Ok((stdout, stderr, status)) => Ran {
            code: status.ok().and_then(|s| s.code()),
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            elapsed: started.elapsed(),
            timed_out: false,
        },
        Err(_) => {
            let _ = child.kill().await;
            Ran {
                code: None,
                stdout: Vec::new(),
                stderr: format!("killed after {timeout:?}"),
                elapsed: started.elapsed(),
                timed_out: true,
            }
        }
    };
    crate::run::raw::record_tool(label, args, &ran);
    Ok(ran)
}

/// The first line of `tool --version`, or `None` when it is not installed.
pub async fn version(tool: Tool) -> Option<String> {
    let arg = match tool {
        Tool::Donsetch => "-V",
        _ => "--version",
    };
    let path = locate(tool)?;
    let mut cmd = tokio::process::Command::new(&path);
    cmd.arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let out = tokio::time::timeout(Duration::from_secs(15), cmd.output())
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned()
        + &String::from_utf8_lossy(&out.stderr);
    // Decorated banners: drop leading symbols and the build target.
    let first = text
        .lines()
        .map(|l| l.trim().trim_start_matches(|c: char| !c.is_alphanumeric()).trim())
        .find(|l| !l.is_empty())?;
    let first = first.split_whitespace().filter(|w| !w.contains("-pc-") && !w.contains("-apple-") && !w.contains("-linux-")).collect::<Vec<_>>().join(" ");
    let first = first.as_str();
    Some(if first.starts_with("Usage") {
        "installed".to_string()
    } else {
        first.to_string()
    })
}

static CONTRACT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A tool answered in a shape OmniSearch does not expect (a missing field,
/// an unknown exit code, a crash): logged loudly, kept for the run summary,
/// and the caller falls through to the next tier.
pub fn contract_warning(tool: &str, what: &str) {
    let line = format!("{tool}: {what}");
    tracing::warn!("CONTRACT: {line}");
    let mut list = CONTRACT.lock().unwrap_or_else(|e| e.into_inner());
    if !list.contains(&line) {
        list.push(line);
    }
}

/// Every contract warning raised so far in this process.
pub fn contract_warnings() -> Vec<String> {
    CONTRACT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
