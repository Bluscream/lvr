//! Process discovery, launching and stopping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

use crate::config::AutostartEntry;

/// One process as seen by a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// Executable name plus full command line, lowercased, for substring matching.
    pub haystack: String,
}

/// Immutable snapshot of the process table.
#[derive(Debug, Clone, Default)]
pub struct ProcSnapshot {
    pub procs: Vec<ProcInfo>,
}

impl ProcSnapshot {
    /// Pids whose command line contains any of `patterns` (already lowercased),
    /// excluding `exclude`.
    pub fn matching(&self, patterns: &[String], exclude: &[u32]) -> Vec<u32> {
        if patterns.is_empty() {
            return Vec::new();
        }
        self.procs
            .iter()
            .filter(|p| !exclude.contains(&p.pid))
            .filter(|p| patterns.iter().any(|pat| p.haystack.contains(pat)))
            .map(|p| p.pid)
            .collect()
    }

    pub fn any_matching(&self, patterns: &[String], exclude: &[u32]) -> bool {
        !self.matching(patterns, exclude).is_empty()
    }

    /// Recursively append all processes whose parent or ancestor is in `pids`.
    pub fn expand_children(&self, pids: &mut Vec<u32>, exclude: &[u32]) {
        let mut added = true;
        while added {
            added = false;
            for p in &self.procs {
                if exclude.contains(&p.pid) || pids.contains(&p.pid) {
                    continue;
                }
                if let Some(ppid) = p.ppid {
                    if pids.contains(&ppid) {
                        pids.push(p.pid);
                        added = true;
                    }
                }
            }
        }
    }
}

/// Repeatedly scans the process table, reusing one `System` for efficiency.
pub struct ProcessScanner {
    system: System,
    self_pid: u32,
}

impl ProcessScanner {
    pub fn new() -> Self {
        let refresh = RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );
        Self {
            system: System::new_with_specifics(refresh),
            self_pid: std::process::id(),
        }
    }

    pub fn scan(&mut self) -> ProcSnapshot {
        let refresh = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::OnlyIfNotSet);
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);

        let procs = self
            .system
            .processes()
            .iter()
            // Threads share their process' command line, so a single Electron
            // app would otherwise show up as a hundred "processes" — and get a
            // hundred signals when stopped.
            .filter(|(_, proc)| proc.thread_kind().is_none())
            .map(|(pid, proc)| {
                let mut haystack = proc.name().to_string_lossy().to_lowercase();
                if let Some(exe) = proc.exe() {
                    haystack.push(' ');
                    haystack.push_str(&exe.to_string_lossy().to_lowercase());
                }
                for arg in proc.cmd() {
                    haystack.push(' ');
                    haystack.push_str(&arg.to_string_lossy().to_lowercase());
                }
                ProcInfo {
                    pid: pid.as_u32(),
                    ppid: proc.parent().map(|p| p.as_u32()),
                    haystack,
                }
            })
            .filter(|p| p.pid != self.self_pid)
            .collect();
        ProcSnapshot { procs }
    }
}

impl Default for ProcessScanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Only pids that address exactly one process are ever signalled: `kill(0)`
/// hits our own process group and `kill(-1)` hits *everything* we may signal,
/// so anything outside `1..=i32::MAX` is rejected outright.
fn signalable(pid: u32) -> Option<libc::pid_t> {
    (pid > 0 && pid <= i32::MAX as u32).then_some(pid as libc::pid_t)
}

/// Is this pid a live process?
///
/// A process that has exited but not been reaped still answers `kill(pid, 0)`,
/// so zombies are filtered out via `/proc` — otherwise every child we
/// terminate would look stubborn and get an unnecessary SIGKILL.
pub fn is_alive(pid: u32) -> bool {
    if signalable(pid).is_none() {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Layout: `pid (comm) state ...`; comm may itself contain spaces and
    // parentheses, so scan back from the last ')'.
    let Some(after_comm) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
        return true;
    };
    after_comm
        .split_whitespace()
        .next()
        .map(|state| state != "Z" && state != "X")
        .unwrap_or(false)
}

fn signal(pid: u32, sig: libc::c_int) -> bool {
    let Some(pid) = signalable(pid) else {
        return false;
    };
    // SAFETY: `pid` is a single, positive process id; a stale one simply
    // returns ESRCH, which we surface as `false`.
    unsafe { libc::kill(pid, sig) == 0 }
}

pub fn terminate(pid: u32) -> bool {
    signal(pid, libc::SIGTERM)
}

pub fn kill(pid: u32) -> bool {
    signal(pid, libc::SIGKILL)
}

/// Send SIGTERM to every pid, wait up to `grace`, then SIGKILL the survivors.
/// Returns the pids that had to be force-killed.
pub async fn stop_pids(pids: &[u32], grace: Duration) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    for &pid in pids {
        terminate(pid);
    }

    let deadline = std::time::Instant::now() + grace;
    loop {
        if !pids.iter().copied().any(is_alive) {
            return Vec::new();
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let stubborn: Vec<u32> = pids.iter().copied().filter(|&p| is_alive(p)).collect();
    for &pid in &stubborn {
        kill(pid);
    }
    stubborn
}

/// The terminal emulators we know how to drive, in order of preference.
/// `{cmd}` is replaced with a shell-quoted `sh -c` payload.
const TERMINALS: &[(&str, &str)] = &[
    ("konsole", "konsole -e sh -c {cmd}"),
    ("ptyxis", "ptyxis -- sh -c {cmd}"),
    ("kgx", "kgx -- sh -c {cmd}"),
    ("gnome-terminal", "gnome-terminal -- sh -c {cmd}"),
    ("ghostty", "ghostty -e sh -c {cmd}"),
    ("wezterm", "wezterm start -- sh -c {cmd}"),
    ("alacritty", "alacritty -e sh -c {cmd}"),
    ("kitty", "kitty sh -c {cmd}"),
    ("foot", "foot sh -c {cmd}"),
    ("rio", "rio -e sh -c {cmd}"),
    ("xfce4-terminal", "xfce4-terminal -x sh -c {cmd}"),
    ("xterm", "xterm -e sh -c {cmd}"),
];

/// First terminal template whose binary exists on `PATH`.
pub fn detect_terminal() -> Option<&'static str> {
    TERMINALS
        .iter()
        .find(|(binary, _)| which(binary).is_some())
        .map(|(_, template)| *template)
}

/// Look a binary up on `PATH`, preferring `/usr/bin` so distro tools win over
/// whatever a user's Homebrew/Nix prefix shadowed them with.
pub fn which(binary: &str) -> Option<PathBuf> {
    let path = if let Some(rest) = binary.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(rest)
        } else {
            PathBuf::from(binary)
        }
    } else {
        PathBuf::from(binary)
    };

    if path.is_absolute() || path.components().count() > 1 {
        return is_executable(&path).then_some(path);
    }
    let preferred = PathBuf::from("/usr/bin").join(binary);
    if is_executable(&preferred) {
        return Some(preferred);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|candidate| is_executable(candidate))
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Turn an entry into the argv we will actually execute.
pub fn build_argv(entry: &AutostartEntry, terminal_template: &str) -> Result<Vec<String>> {
    let command = entry.command.trim();
    if command.is_empty() {
        bail!("no command configured");
    }

    if entry.console {
        let template = if terminal_template.trim().is_empty() {
            detect_terminal().ok_or_else(|| {
                anyhow!("no terminal emulator found; set general.terminal in the config")
            })?
        } else {
            terminal_template.trim()
        };
        if !template.contains("{cmd}") {
            bail!("terminal template must contain {{cmd}}");
        }
        let payload = shell_words::quote(command).into_owned();
        let rendered = template.replace("{cmd}", &payload);
        let argv = shell_words::split(&rendered)
            .with_context(|| format!("parsing terminal command `{rendered}`"))?;
        if argv.is_empty() {
            bail!("terminal template produced an empty command");
        }
        return Ok(argv);
    }

    if entry.use_shell {
        return Ok(vec!["sh".into(), "-c".into(), command.to_string()]);
    }

    let argv =
        shell_words::split(command).with_context(|| format!("parsing command `{command}`"))?;
    if argv.is_empty() {
        bail!("command is empty after parsing");
    }
    Ok(argv)
}

/// Spawn an entry. Output is discarded (console entries get their own window).
pub fn launch(entry: &AutostartEntry, terminal_template: &str) -> Result<Child> {
    let argv = build_argv(entry, terminal_template)?;
    let program = resolve_program(&argv[0])?;

    let mut command = Command::new(&program);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let dir = entry.working_dir.trim();
    if !dir.is_empty() {
        let dir = PathBuf::from(dir);
        if !dir.is_dir() {
            bail!("working directory `{}` does not exist", dir.display());
        }
        command.current_dir(dir);
    }

    command
        .spawn()
        .with_context(|| format!("launching `{}`", argv.join(" ")))
}

fn resolve_program(program: &str) -> Result<PathBuf> {
    which(program).ok_or_else(|| anyhow!("`{program}` not found (not on PATH or not executable)"))
}

/// Run a short-lived helper command and wait for it, returning stdout.
pub async fn run_capture(argv: &[String]) -> Result<String> {
    let Some((program, args)) = argv.split_first() else {
        bail!("empty command");
    };
    let program = resolve_program(program)?;
    let output = tokio::process::Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("running `{}`", argv.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("`{}` failed: {}", argv.join(" "), stderr);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a command line (shell-word split) and wait for it to finish.
pub async fn run_command_line(line: &str) -> Result<()> {
    let argv = shell_words::split(line).with_context(|| format!("parsing `{line}`"))?;
    if argv.is_empty() {
        bail!("empty command");
    }
    run_capture(&argv).await.map(|_| ())
}

/// Detached spawn of a command line, used for WiVRn and other one-shots.
pub fn spawn_command_line(line: &str) -> Result<Child> {
    let argv = shell_words::split(line).with_context(|| format!("parsing `{line}`"))?;
    let Some((program, args)) = argv.split_first() else {
        bail!("empty command");
    };
    let program = resolve_program(program)?;
    Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launching `{line}`"))
}

/// Reaps finished children so they do not linger as zombies.
#[derive(Default)]
pub struct ChildRegistry {
    children: HashMap<String, Child>,
}

impl ChildRegistry {
    pub fn insert(&mut self, id: &str, child: Child) {
        if let Some(mut previous) = self.children.insert(id.to_string(), child) {
            let _ = previous.try_wait();
        }
    }

    /// Pid of the still-running child for `id`, reaping it if it has exited.
    pub fn live_pid(&mut self, id: &str) -> Option<u32> {
        let child = self.children.get_mut(id)?;
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                self.children.remove(id);
                None
            }
            Ok(None) => Some(child.id()),
        }
    }

    pub fn forget(&mut self, id: &str) {
        if let Some(mut child) = self.children.remove(id) {
            let _ = child.try_wait();
        }
    }

    /// Reap every finished child; call once per tick.
    pub fn reap(&mut self) {
        self.children.retain(|_, child| match child.try_wait() {
            Ok(Some(_)) | Err(_) => false,
            Ok(None) => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutostartEntry;

    fn snapshot() -> ProcSnapshot {
        ProcSnapshot {
            procs: vec![
                ProcInfo {
                    pid: 10,
                    ppid: None,
                    haystack: "vrchat.exe z:\\games\\vrchat\\vrchat.exe --no-vr".into(),
                },
                ProcInfo {
                    pid: 11,
                    ppid: None,
                    haystack: "slimevr /app/main/slimevr".into(),
                },
                ProcInfo {
                    pid: 12,
                    ppid: None,
                    haystack: "bash /home/blu/.local/bin/vrcosc".into(),
                },
                ProcInfo {
                    pid: 13,
                    ppid: Some(12),
                    haystack: "node --disable-warning=experimentalwarning server.ts".into(),
                },
            ],
        }
    }

    #[test]
    fn expand_children_finds_child_processes() {
        let snap = snapshot();
        let mut pids = vec![12];
        snap.expand_children(&mut pids, &[]);
        assert_eq!(pids, vec![12, 13]);
    }

    #[test]
    fn matching_finds_substrings_and_honours_exclusions() {
        let snap = snapshot();
        assert_eq!(snap.matching(&["vrchat.exe".into()], &[]), vec![10]);
        assert_eq!(snap.matching(&["vrcosc".into()], &[]), vec![12]);
        assert!(snap.matching(&["vrcosc".into()], &[12]).is_empty());
        assert!(snap.matching(&[], &[]).is_empty());
        assert!(snap.any_matching(&["slimevr".into()], &[]));
        assert!(!snap.any_matching(&["nothing-here".into()], &[]));
    }

    #[test]
    fn matching_is_case_insensitive_via_lowercased_patterns() {
        let snap = snapshot();
        // Patterns arrive already lowercased from `effective_patterns`.
        assert_eq!(snap.matching(&["vrchat".into()], &[]), vec![10]);
    }

    #[test]
    fn build_argv_splits_plain_commands() {
        let entry = AutostartEntry {
            command: "/usr/bin/foo --bar 'baz qux'".into(),
            ..Default::default()
        };
        assert_eq!(
            build_argv(&entry, "").unwrap(),
            vec!["/usr/bin/foo", "--bar", "baz qux"]
        );
    }

    #[test]
    fn build_argv_uses_sh_when_requested() {
        let entry = AutostartEntry {
            command: "foo && bar".into(),
            use_shell: true,
            ..Default::default()
        };
        assert_eq!(
            build_argv(&entry, "").unwrap(),
            vec!["sh", "-c", "foo && bar"]
        );
    }

    #[test]
    fn build_argv_wraps_console_entries_in_the_terminal() {
        let entry = AutostartEntry {
            command: "/home/blu/Desktop/VRCVideoCacher".into(),
            console: true,
            ..Default::default()
        };
        assert_eq!(
            build_argv(&entry, "konsole -e sh -c {cmd}").unwrap(),
            vec![
                "konsole",
                "-e",
                "sh",
                "-c",
                "/home/blu/Desktop/VRCVideoCacher"
            ]
        );
    }

    #[test]
    fn build_argv_quotes_console_commands_with_spaces() {
        let entry = AutostartEntry {
            command: "/opt/My App/run --flag".into(),
            console: true,
            ..Default::default()
        };
        let argv = build_argv(&entry, "konsole -e sh -c {cmd}").unwrap();
        assert_eq!(argv.last().unwrap(), "/opt/My App/run --flag");
    }

    #[test]
    fn build_argv_rejects_bad_input() {
        let empty = AutostartEntry::default();
        assert!(build_argv(&empty, "").is_err());

        let entry = AutostartEntry {
            command: "foo".into(),
            console: true,
            ..Default::default()
        };
        assert!(build_argv(&entry, "konsole -e sh -c").is_err());

        let unbalanced = AutostartEntry {
            command: "foo 'unbalanced".into(),
            ..Default::default()
        };
        assert!(build_argv(&unbalanced, "").is_err());
    }

    #[test]
    fn which_finds_core_utilities_and_rejects_nonsense() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn which_expands_tilde_paths() {
        if let Some(home) = std::env::var_os("HOME") {
            let tilde_path = "~/bin/definitely-not-a-real-binary-xyz";
            let expected = PathBuf::from(home).join("bin/definitely-not-a-real-binary-xyz");
            let resolved = which(tilde_path);
            assert_eq!(resolved, is_executable(&expected).then_some(expected));
        }
    }

    #[test]
    fn is_alive_reports_our_own_process() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn broadcast_pids_are_never_signalled() {
        // 0 = our whole process group, -1 (u32::MAX) = every process we may
        // signal. Both must be rejected before they reach `kill`.
        for pid in [0, u32::MAX, i32::MAX as u32 + 1] {
            assert!(signalable(pid).is_none(), "{pid} must not be signalable");
            assert!(!is_alive(pid));
            assert!(!terminate(pid));
            assert!(!kill(pid));
        }
        assert_eq!(signalable(1234), Some(1234));
    }

    #[test]
    fn zombies_do_not_count_as_alive() {
        // Exited but deliberately not reaped: `kill(pid, 0)` still succeeds.
        let mut child = Command::new("true").spawn().expect("spawn true");
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!is_alive(pid), "a zombie must not look alive");
        let _ = child.wait();
    }

    #[tokio::test]
    async fn stop_pids_terminates_a_child() {
        let child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let forced = stop_pids(&[pid], Duration::from_secs(3)).await;
        assert!(forced.is_empty(), "sleep should honour SIGTERM");
        // Reap so the child does not linger as a zombie for the rest of the run.
        let mut child = child;
        let _ = child.wait();
    }

    #[test]
    fn child_registry_reaps_exited_children() {
        let mut registry = ChildRegistry::default();
        let child = Command::new("true").spawn().expect("spawn true");
        registry.insert("t", child);
        // Give the child a moment to exit before we poll it.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(registry.live_pid("t"), None);
        assert_eq!(registry.live_pid("missing"), None);
    }
}
